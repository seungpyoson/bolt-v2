//! Fail-closed provider binding for `HYPERLIQUID`.
//!
//! This module registers the provider key and NT crate boundary. Execution
//! mapping stays gated behind SSM-resolved credentials, explicit TOML runtime
//! fields, and a consumed surface-bound live-submit approval.

use std::{
    any::Any,
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
};

use futures_util::future::{BoxFuture, FutureExt};
use nautilus_core::string::secret::REDACTED;
use nautilus_hyperliquid::{
    common::enums::HyperliquidEnvironment as NtHyperliquidEnvironment,
    config::{HyperliquidDataClientConfig, HyperliquidExecClientConfig},
    factories::{
        HyperliquidDataClientFactory, HyperliquidExecFactoryConfig,
        HyperliquidExecutionClientFactory,
    },
    http::client::HyperliquidHttpClient,
};
use nautilus_model::identifiers::{AccountId, InstrumentId};
use nautilus_network::websocket::TransportBackend;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::bolt_v3_providers::{ProviderExclusiveSignerOwner, ProviderResolvedSecrets};
use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3ClientAdapterConfig, BoltV3DataClientAdapterConfig,
        BoltV3ExecutionClientAdapterConfig,
    },
    bolt_v3_config::ClientBlock,
    bolt_v3_config::resolve_root_relative_path,
    bolt_v3_market_families::{
        MarketIdentityPlan, hyperliquid_instrument, market_identity_plan_from_config,
        outcome_group, updown,
    },
    bolt_v3_operator_artifacts::{WrittenOperatorArtifact, is_lowercase_sha256, read_file_bounded},
    bolt_v3_providers::hyperliquid_artifacts::{
        HyperliquidLiveSubmitApprovalBinding, HyperliquidLiveSubmitApprovalInput,
        HyperliquidLiveSubmitOrderLimits, HyperliquidProductSubmitProofArtifactInput,
        HyperliquidProductSubmitProofBinding, HyperliquidProductSubmitProofEvidenceRef,
        consume_hyperliquid_live_submit_approval_artifact,
        persist_consumed_hyperliquid_live_submit_approval_artifact,
        read_hyperliquid_live_submit_approval_artifact,
        validate_hyperliquid_live_submit_approval_artifact,
        validate_hyperliquid_product_submit_proof_artifact_bytes,
        write_hyperliquid_live_submit_approval_artifact,
        write_hyperliquid_product_submit_proof_artifact,
    },
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderLiveSubmitApproval,
        ProviderLiveSubmitApprovalContext, ProviderLiveSubmitApprovals,
        ProviderLiveSubmitArmingPreflight, ProviderLiveSubmitOrderLimits,
        ProviderProductSubmitProofArtifactRequest, ProviderSecretRequirement,
        ProviderSecretResolveContext, ProviderSharedSignerOwnerContext, ProviderSsmPathReference,
        ResolvedClientSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
    strategies::registry::FeeProvider,
};

use super::hyperliquid_artifacts::HyperliquidLiveSubmitApprovalConsumption;

pub const KEY: &str = "HYPERLIQUID";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] =
    &[updown::KEY, hyperliquid_instrument::KEY, outcome_group::KEY];
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
pub struct HyperliquidDataConfig {
    pub environment: HyperliquidEnvironment,
    pub base_url_ws: String,
    pub base_url_http: String,
    pub proxy_url: Option<String>,
    pub http_timeout_secs: u64,
    pub ws_timeout_secs: u64,
    pub update_instruments_interval_mins: u64,
    pub transport_backend: TransportBackend,
}

pub fn metadata_refresh_interval_mins(client: &ClientBlock) -> Result<Option<u64>, String> {
    let Some(data) = client.data.as_ref() else {
        return Ok(None);
    };
    let data = data
        .clone()
        .try_into::<HyperliquidDataConfig>()
        .map_err(|error| error.to_string())?;
    Ok(Some(data.update_instruments_interval_mins))
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
    pub live_submit_approval_artifact_path: Option<String>,
    pub live_submit_approval_artifact_max_bytes: Option<u64>,
    pub live_submit_max_order_count: Option<u32>,
    pub live_submit_max_order_notional: Option<String>,
    pub live_submit_product_proof_artifact_path: Option<String>,
    pub live_submit_product_proof_artifact_sha256: Option<String>,
    pub live_submit_product_proof_artifact_max_bytes: Option<u64>,
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
    pub include_builder_attribution: bool,
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
    ApprovalGated,
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
const APPROVAL_GATED_MISSING_SUBMIT_PROOF: &[&str] = &[];
const SPOT_DISCOVERY_SOURCES: &[&str] = &[
    "nautilus_hyperliquid::http::query::InfoRequest::spot_meta",
    "nautilus_hyperliquid::http::models::SpotMeta",
    "nautilus_hyperliquid::http::parse::parse_spot_instruments",
];
const SPOT_OFFICIAL_DOCUMENTATION_SOURCES: &[&str] =
    &["Hyperliquid Info endpoint spot metadata `spotMeta`"];
const HIP3_DISCOVERY_SOURCES: &[&str] = &[
    "nautilus_hyperliquid::http::query::InfoRequest::all_perp_metas",
    "nautilus_hyperliquid::http::models::PerpMeta",
    "nautilus_hyperliquid::http::parse::parse_perp_instruments",
];
const HIP3_OFFICIAL_DOCUMENTATION_SOURCES: &[&str] =
    &["Hyperliquid Info endpoint all perp dex metadata `allPerpMetas`"];
const HIP4_DISCOVERY_SOURCES: &[&str] = &[
    "nautilus_hyperliquid::http::query::InfoRequest::outcome_meta",
    "nautilus_hyperliquid::http::models::OutcomeMeta",
    "nautilus_hyperliquid::http::parse::parse_outcome_instruments",
];
const HIP4_OFFICIAL_DOCUMENTATION_SOURCES: &[&str] =
    &["Hyperliquid Info endpoint outcome metadata `outcomeMeta`"];

const HYPERLIQUID_PRODUCT_MATRIX: &[HyperliquidProductMatrixEntry] = &[
    HyperliquidProductMatrixEntry {
        provider_key: KEY,
        product_surface: HyperliquidProductSurface::StandardPerps,
        discovery_status: HyperliquidDiscoveryStatus::Supported,
        discovery_sources: STANDARD_PERPS_DISCOVERY_SOURCES,
        official_documentation_sources: STANDARD_PERPS_OFFICIAL_DOCUMENTATION_SOURCES,
        live_submit_status: HyperliquidSubmitStatus::ApprovalGated,
        missing_submit_proof: APPROVAL_GATED_MISSING_SUBMIT_PROOF,
    },
    HyperliquidProductMatrixEntry {
        provider_key: KEY,
        product_surface: HyperliquidProductSurface::Spot,
        discovery_status: HyperliquidDiscoveryStatus::Supported,
        discovery_sources: SPOT_DISCOVERY_SOURCES,
        official_documentation_sources: SPOT_OFFICIAL_DOCUMENTATION_SOURCES,
        live_submit_status: HyperliquidSubmitStatus::ApprovalGated,
        missing_submit_proof: APPROVAL_GATED_MISSING_SUBMIT_PROOF,
    },
    HyperliquidProductMatrixEntry {
        provider_key: KEY,
        product_surface: HyperliquidProductSurface::Hip3BuilderPerps,
        discovery_status: HyperliquidDiscoveryStatus::Supported,
        discovery_sources: HIP3_DISCOVERY_SOURCES,
        official_documentation_sources: HIP3_OFFICIAL_DOCUMENTATION_SOURCES,
        live_submit_status: HyperliquidSubmitStatus::ApprovalGated,
        missing_submit_proof: APPROVAL_GATED_MISSING_SUBMIT_PROOF,
    },
    HyperliquidProductMatrixEntry {
        provider_key: KEY,
        product_surface: HyperliquidProductSurface::Hip4Outcomes,
        discovery_status: HyperliquidDiscoveryStatus::Supported,
        discovery_sources: HIP4_DISCOVERY_SOURCES,
        official_documentation_sources: HIP4_OFFICIAL_DOCUMENTATION_SOURCES,
        live_submit_status: HyperliquidSubmitStatus::ApprovalGated,
        missing_submit_proof: APPROVAL_GATED_MISSING_SUBMIT_PROOF,
    },
];

pub fn hyperliquid_product_matrix() -> &'static [HyperliquidProductMatrixEntry] {
    HYPERLIQUID_PRODUCT_MATRIX
}

pub const USER_FEES_OFFICIAL_INFO_REQUEST_WEIGHT: u32 = 20;
pub const REST_EGRESS_CAP_PER_MINUTE: u32 = 1200;
pub const MAX_REST_REQUESTS_PER_ORDER_COMMAND: u32 = USER_FEES_OFFICIAL_INFO_REQUEST_WEIGHT;
pub const USER_FEES_OFFICIAL_RATE_LIMIT_SOURCE: &str =
    "Hyperliquid Docs: Rate limits and user limits - all other documented info requests weight 20";
pub const USER_FEES_NT_CALLERS: &[&str] = &[
    "nautilus_hyperliquid::http::query::InfoRequest::user_fees",
    "nautilus_hyperliquid::http::client::InnerHyperliquidHttpClient::info_user_fees",
    "nautilus_hyperliquid::http::client::HyperliquidHttpClient::info_user_fees",
    "nautilus_hyperliquid::python::http::HyperliquidHttpClient::py_info_user_fees",
];
const USER_CROSS_RATE_FIELD: &str = "userCrossRate";
const BASIS_POINTS_PER_UNIT_RATE: i64 = 10_000;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HyperliquidUserFeesRequestWeightStatus {
    OfficialWeightAccounted,
    OfficialWeightAccountedByBoltProviderPolicy,
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
    } else if hyperliquid_provider_policy_accounts_official_user_fees_weight() {
        HyperliquidUserFeesRequestWeightStatus::OfficialWeightAccountedByBoltProviderPolicy
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

fn hyperliquid_provider_policy_accounts_official_user_fees_weight() -> bool {
    MAX_REST_REQUESTS_PER_ORDER_COMMAND >= USER_FEES_OFFICIAL_INFO_REQUEST_WEIGHT
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

pub fn hyperliquid_live_submit_signer_fingerprint(private_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"bolt-v3-hyperliquid-live-submit-signer-v1:");
    let normalized = private_key
        .strip_prefix("0x")
        .or_else(|| private_key.strip_prefix("0X"))
        .unwrap_or(private_key);
    for &byte in normalized.as_bytes() {
        hasher.update([byte.to_ascii_lowercase()]);
    }
    hex::encode(hasher.finalize())
}

pub fn validate_client(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if client.data.is_none() && client.execution.is_none() {
        errors.push(format!(
            "clients.{key} (provider={KEY}) must declare a proven [data] or [execution] block before Hyperliquid can be used"
        ));
    }
    if let Some(data) = &client.data {
        match data.clone().try_into::<HyperliquidDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_config(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.data: {message}")),
        }
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

fn validate_data_config(key: &str, data: &HyperliquidDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    errors.extend(validate_url_field(
        key,
        "data.base_url_ws",
        data.base_url_ws.as_str(),
        &["ws", "wss"],
    ));
    errors.extend(validate_url_field(
        key,
        "data.base_url_http",
        data.base_url_http.as_str(),
        &["http", "https"],
    ));
    if let Some(proxy_url) = &data.proxy_url {
        errors.extend(validate_url_field(
            key,
            "data.proxy_url",
            proxy_url.as_str(),
            &["http", "https", "socks5", "socks5h"],
        ));
    }
    let positive_fields: &[(&str, u64)] = &[
        ("http_timeout_secs", data.http_timeout_secs),
        ("ws_timeout_secs", data.ws_timeout_secs),
        (
            "update_instruments_interval_mins",
            data.update_instruments_interval_mins,
        ),
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "clients.{key}.data.{field} must be a positive integer"
            ));
        }
    }
    errors
}

fn validate_url_field(
    key: &str,
    field: &str,
    value: &str,
    allowed_schemes: &[&str],
) -> Vec<String> {
    let mut errors = Vec::new();
    if value.trim().is_empty() {
        errors.push(format!("clients.{key}.{field} must be a non-empty URL"));
        return errors;
    }
    let Ok(parsed) = Url::parse(value) else {
        errors.push(format!("clients.{key}.{field} must be a valid URL"));
        return errors;
    };
    if !allowed_schemes.contains(&parsed.scheme())
        || !value[parsed.scheme().len()..].starts_with("://")
        || !parsed.has_host()
    {
        errors.push(format!("clients.{key}.{field} must be a valid URL"));
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
        if execution.product_surfaces.len() != 1 {
            errors.push(format!(
                "clients.{key}.execution.product_surfaces must select exactly one Hyperliquid product surface when live_submit_approval_id is configured"
            ));
        }
        errors.extend(validate_live_submit_approval_config(key, execution));
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

fn validate_live_submit_approval_config(
    key: &str,
    execution: &HyperliquidExecutionConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    match execution.live_submit_approval_artifact_path.as_deref() {
        Some(path) if !path.trim().is_empty() => {}
        _ => errors.push(format!(
            "clients.{key}.execution.live_submit_approval_artifact_path is required when live_submit_approval_id is configured"
        )),
    }
    match execution.live_submit_approval_artifact_max_bytes {
        Some(value) if value > 0 => {}
        _ => errors.push(format!(
            "clients.{key}.execution.live_submit_approval_artifact_max_bytes must be positive when live_submit_approval_id is configured"
        )),
    }
    match execution.live_submit_max_order_count {
        Some(value) if value > 0 => {}
        _ => errors.push(format!(
            "clients.{key}.execution.live_submit_max_order_count must be positive when live_submit_approval_id is configured"
        )),
    }
    match execution.live_submit_max_order_notional.as_deref() {
        Some(value) => match Decimal::from_str(value.trim()) {
            Ok(value) if value > Decimal::ZERO => {}
            _ => errors.push(format!(
                "clients.{key}.execution.live_submit_max_order_notional must be positive decimal text when live_submit_approval_id is configured"
            )),
        },
        None => errors.push(format!(
            "clients.{key}.execution.live_submit_max_order_notional is required when live_submit_approval_id is configured"
        )),
    }
    match execution.live_submit_product_proof_artifact_path.as_deref() {
        Some(path) if !path.trim().is_empty() => {}
        _ => errors.push(format!(
            "clients.{key}.execution.live_submit_product_proof_artifact_path is required when live_submit_approval_id is configured"
        )),
    }
    match execution.live_submit_product_proof_artifact_sha256.as_deref() {
        Some(value) if is_lowercase_sha256(value) => {}
        _ => errors.push(format!(
            "clients.{key}.execution.live_submit_product_proof_artifact_sha256 must be lowercase sha256 when live_submit_approval_id is configured"
        )),
    }
    match execution.live_submit_product_proof_artifact_max_bytes {
        Some(value) if value > 0 => {}
        _ => errors.push(format!(
            "clients.{key}.execution.live_submit_product_proof_artifact_max_bytes must be positive when live_submit_approval_id is configured"
        )),
    }
    errors
}

fn validate_user_fees_request_weight_policy(key: &str) -> Vec<String> {
    let policy = hyperliquid_user_fees_request_weight_policy();
    match policy.status {
        HyperliquidUserFeesRequestWeightStatus::OfficialWeightAccounted
        | HyperliquidUserFeesRequestWeightStatus::OfficialWeightAccountedByBoltProviderPolicy => {
            Vec::new()
        }
        HyperliquidUserFeesRequestWeightStatus::FailClosedPinnedNtWeightMismatch => vec![format!(
            "clients.{key}.execution.live_submit_approval_id cannot enable Hyperliquid live submit while pinned NautilusTrader {} info request weight is {} but the official documented weight is {}; update the NT pin or the provider rate-limit policy before live submit",
            policy.request_type,
            policy.pinned_nt_info_base_weight,
            policy.official_info_request_weight
        )],
    }
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

pub fn allow_shared_signer_owner(context: ProviderSharedSignerOwnerContext<'_>) -> bool {
    if context.existing_client_keys.len() != 1 {
        return false;
    }
    let Some(existing_paths) = configured_signer_ssm_paths(
        context.region,
        context.existing_client_key,
        context.existing_client,
    ) else {
        return false;
    };
    let Some(paths) =
        configured_signer_ssm_paths(context.region, context.client_key, context.client)
    else {
        return false;
    };
    if existing_paths != paths {
        return false;
    }
    let Some(existing_surface) = single_product_surface(context.existing_client) else {
        return false;
    };
    let Some(surface) = single_product_surface(context.client) else {
        return false;
    };
    matches!(
        (existing_surface, surface),
        (
            HyperliquidProductSurface::StandardPerps,
            HyperliquidProductSurface::Spot
        ) | (
            HyperliquidProductSurface::Spot,
            HyperliquidProductSurface::StandardPerps
        )
    )
}

fn configured_signer_ssm_paths(
    region: &str,
    client_key: &str,
    client: &ClientBlock,
) -> Option<(String, String)> {
    let context = ProviderSecretResolveContext {
        client_key,
        region,
        client,
    };
    parse_secrets_config(&context).ok().map(|secrets| {
        (
            secrets.private_key_ssm_path,
            secrets.account_address_ssm_path,
        )
    })
}

fn single_product_surface(client: &ClientBlock) -> Option<HyperliquidProductSurface> {
    let execution = client.execution.clone()?;
    let cfg: HyperliquidExecutionConfig = execution.try_into().ok()?;
    let [surface] = cfg.product_surfaces.as_slice() else {
        return None;
    };
    Some(*surface)
}

pub fn load_live_submit_approval(
    context: ProviderLiveSubmitApprovalContext<'_>,
) -> Result<Option<ProviderLiveSubmitApproval>, anyhow::Error> {
    let Some(execution) = &context.client.execution else {
        return Ok(None);
    };
    let cfg: HyperliquidExecutionConfig = execution.clone().try_into().map_err(|source| {
        hyperliquid_adapter_validation_error(
            context.client_key,
            "execution",
            format!("Hyperliquid execution config is invalid: {source}"),
        )
    })?;
    let Some(expected_approval_id) = cfg.live_submit_approval_id.as_deref() else {
        return Ok(None);
    };
    validate_target_surfaces_for_live_submit_approval(&context, &cfg)?;
    let approval_path = cfg
        .live_submit_approval_artifact_path
        .as_deref()
        .ok_or_else(|| {
            hyperliquid_adapter_validation_error(
                context.client_key,
                "execution.live_submit_approval_artifact_path",
                "Hyperliquid live submit requires a configured approval artifact path",
            )
        })?;
    let approval_max_bytes = cfg.live_submit_approval_artifact_max_bytes.ok_or_else(|| {
        hyperliquid_adapter_validation_error(
            context.client_key,
            "execution.live_submit_approval_artifact_max_bytes",
            "Hyperliquid live submit requires a configured approval artifact byte cap",
        )
    })?;
    let product_proof_max_bytes = cfg
        .live_submit_product_proof_artifact_max_bytes
        .ok_or_else(|| {
            hyperliquid_adapter_validation_error(
                context.client_key,
                "execution.live_submit_product_proof_artifact_max_bytes",
                "Hyperliquid live submit requires a configured product proof artifact byte cap",
            )
        })?;
    let resolved_path = resolve_root_relative_path(&context.loaded.root_path, approval_path);
    let binding = live_submit_approval_binding(&context, &cfg)?;
    validate_product_submit_proof_artifact(
        context.client_key,
        &context.loaded.root_path,
        &binding,
        product_proof_max_bytes,
    )?;
    let mut approval =
        read_hyperliquid_live_submit_approval_artifact(&resolved_path, approval_max_bytes)?;
    let consumed = consume_hyperliquid_live_submit_approval_artifact(
        &mut approval,
        &binding,
        expected_approval_id,
        context.now_unix_seconds,
    )?;
    persist_consumed_hyperliquid_live_submit_approval_artifact(&resolved_path, &approval)?;
    let order_limits = consumed.order_limits();
    let max_order_count = order_limits.max_order_count;
    let max_order_notional =
        Decimal::from_str(order_limits.max_order_notional.trim()).map_err(anyhow::Error::new)?;
    Ok(Some(ProviderLiveSubmitApproval::with_order_limits(
        Box::new(consumed),
        ProviderLiveSubmitOrderLimits {
            max_order_count,
            max_order_notional,
        },
    )))
}

pub fn preflight_live_submit_arming(
    context: ProviderLiveSubmitApprovalContext<'_>,
) -> Result<Option<ProviderLiveSubmitArmingPreflight>, anyhow::Error> {
    let Some(execution) = &context.client.execution else {
        return Ok(None);
    };
    let cfg: HyperliquidExecutionConfig = execution.clone().try_into().map_err(|source| {
        hyperliquid_adapter_validation_error(
            context.client_key,
            "execution",
            format!("Hyperliquid execution config is invalid: {source}"),
        )
    })?;
    let Some(expected_approval_id) = cfg.live_submit_approval_id.as_deref() else {
        return Ok(None);
    };
    validate_target_surfaces_for_live_submit_approval(&context, &cfg)?;
    let approval_path = cfg
        .live_submit_approval_artifact_path
        .as_deref()
        .ok_or_else(|| {
            hyperliquid_adapter_validation_error(
                context.client_key,
                "execution.live_submit_approval_artifact_path",
                "Hyperliquid live submit requires a configured approval artifact path",
            )
        })?;
    let approval_max_bytes = cfg.live_submit_approval_artifact_max_bytes.ok_or_else(|| {
        hyperliquid_adapter_validation_error(
            context.client_key,
            "execution.live_submit_approval_artifact_max_bytes",
            "Hyperliquid live submit requires a configured approval artifact byte cap",
        )
    })?;
    let product_proof_max_bytes = cfg
        .live_submit_product_proof_artifact_max_bytes
        .ok_or_else(|| {
            hyperliquid_adapter_validation_error(
                context.client_key,
                "execution.live_submit_product_proof_artifact_max_bytes",
                "Hyperliquid live submit requires a configured product proof artifact byte cap",
            )
        })?;
    let resolved_path = resolve_root_relative_path(&context.loaded.root_path, approval_path);
    let binding = live_submit_approval_binding(&context, &cfg)?;
    validate_product_submit_proof_artifact(
        context.client_key,
        &context.loaded.root_path,
        &binding,
        product_proof_max_bytes,
    )?;
    let approval =
        read_hyperliquid_live_submit_approval_artifact(&resolved_path, approval_max_bytes)?;
    validate_hyperliquid_live_submit_approval_artifact(
        Some(&approval),
        &binding,
        context.now_unix_seconds,
    )
    .map_err(anyhow::Error::new)?;
    if approval.approval_id != expected_approval_id {
        return Err(hyperliquid_adapter_validation_error(
            context.client_key,
            "execution.live_submit_approval_id",
            "Hyperliquid live-submit approval artifact id does not match configured approval id",
        ));
    }
    Ok(Some(ProviderLiveSubmitArmingPreflight {
        provider_key: KEY,
        client_key: context.client_key.to_string(),
        product_surface: product_surface_name(binding.product_surface).to_string(),
        approval_artifact_path: approval_path.to_string(),
        product_submit_proof_artifact_path: binding.product_submit_proof.artifact_path,
        max_order_count: binding.order_limits.max_order_count,
        max_order_notional: binding.order_limits.max_order_notional,
    }))
}

pub fn write_configured_live_submit_approval_artifact(
    context: ProviderLiveSubmitApprovalContext<'_>,
    expires_at_unix_seconds: u64,
) -> Result<WrittenOperatorArtifact, anyhow::Error> {
    let Some(execution) = &context.client.execution else {
        return Err(hyperliquid_adapter_validation_error(
            context.client_key,
            "execution",
            "Hyperliquid live-submit approval materialization requires [execution]",
        ));
    };
    let cfg: HyperliquidExecutionConfig = execution.clone().try_into().map_err(|source| {
        hyperliquid_adapter_validation_error(
            context.client_key,
            "execution",
            format!("Hyperliquid execution config is invalid: {source}"),
        )
    })?;
    let approval_id = cfg.live_submit_approval_id.as_deref().ok_or_else(|| {
        hyperliquid_adapter_validation_error(
            context.client_key,
            "execution.live_submit_approval_id",
            "Hyperliquid approval materialization requires configured live_submit_approval_id",
        )
    })?;
    let approval_path = cfg
        .live_submit_approval_artifact_path
        .as_deref()
        .ok_or_else(|| {
            hyperliquid_adapter_validation_error(
                context.client_key,
                "execution.live_submit_approval_artifact_path",
                "Hyperliquid approval materialization requires configured approval artifact path",
            )
        })?;
    match cfg.live_submit_approval_artifact_max_bytes {
        Some(value) if value > 0 => {}
        _ => {
            return Err(hyperliquid_adapter_validation_error(
                context.client_key,
                "execution.live_submit_approval_artifact_max_bytes",
                "Hyperliquid approval materialization requires configured approval artifact byte cap",
            ));
        }
    }
    if expires_at_unix_seconds <= context.now_unix_seconds {
        return Err(hyperliquid_adapter_validation_error(
            context.client_key,
            "execution.live_submit_approval_id",
            "Hyperliquid approval materialization requires expires_at_unix_seconds after the current time",
        ));
    }
    let resolved_path = resolve_root_relative_path(&context.loaded.root_path, approval_path);
    let binding = live_submit_approval_binding(&context, &cfg)?;
    write_hyperliquid_live_submit_approval_artifact(
        HyperliquidLiveSubmitApprovalInput {
            approval_id: approval_id.to_string(),
            base_sha: binding.base_sha,
            provider_id: binding.provider_id,
            product_surface: binding.product_surface,
            toml_checksum: binding.toml_checksum,
            signer_fingerprint: binding.signer_fingerprint,
            order_limits: binding.order_limits,
            product_submit_proof: binding.product_submit_proof,
            expires_at: expires_at_unix_seconds,
            used_at: None,
        },
        &resolved_path,
    )
    .map_err(anyhow::Error::new)
}

pub fn write_product_submit_proof_artifact(
    request: ProviderProductSubmitProofArtifactRequest<'_>,
) -> Result<WrittenOperatorArtifact, anyhow::Error> {
    let product_surface = parse_product_surface_name(request.product_surface).ok_or_else(|| {
        anyhow::anyhow!("unsupported product_surface `{}`", request.product_surface)
    })?;
    write_hyperliquid_product_submit_proof_artifact(
        HyperliquidProductSubmitProofArtifactInput {
            provider_id: request.provider_id.to_string(),
            product_surface,
            toml_checksum: request.toml_checksum.to_string(),
            order_proof: product_submit_proof_evidence_ref(request.order_proof),
            fill_proof: product_submit_proof_evidence_ref(request.fill_proof),
            rounding_proof: product_submit_proof_evidence_ref(request.rounding_proof),
            fee_proof: product_submit_proof_evidence_ref(request.fee_proof),
            settlement_proof: request
                .settlement_proof
                .map(product_submit_proof_evidence_ref),
        },
        request.output_path,
    )
    .map_err(anyhow::Error::new)
}

fn product_submit_proof_evidence_ref(
    reference: crate::bolt_v3_providers::ProviderArtifactReference<'_>,
) -> HyperliquidProductSubmitProofEvidenceRef {
    HyperliquidProductSubmitProofEvidenceRef {
        artifact_path: reference.artifact_path.to_string(),
        artifact_sha256: reference.artifact_sha256.to_string(),
    }
}

fn live_submit_approval_binding(
    context: &ProviderLiveSubmitApprovalContext<'_>,
    cfg: &HyperliquidExecutionConfig,
) -> Result<HyperliquidLiveSubmitApprovalBinding, anyhow::Error> {
    let [product_surface] = cfg.product_surfaces.as_slice() else {
        return Err(hyperliquid_adapter_validation_error(
            context.client_key,
            "execution.product_surfaces",
            "Hyperliquid live submit requires exactly one product surface per execution client",
        ));
    };
    let secrets = secrets_for(context.client_key, context.resolved)?;
    let max_order_count = cfg.live_submit_max_order_count.ok_or_else(|| {
        hyperliquid_adapter_validation_error(
            context.client_key,
            "execution.live_submit_max_order_count",
            "Hyperliquid live submit requires configured max order count",
        )
    })?;
    let max_order_notional = cfg.live_submit_max_order_notional.clone().ok_or_else(|| {
        hyperliquid_adapter_validation_error(
            context.client_key,
            "execution.live_submit_max_order_notional",
            "Hyperliquid live submit requires configured max order notional",
        )
    })?;
    let product_proof_artifact_path = cfg
        .live_submit_product_proof_artifact_path
        .clone()
        .ok_or_else(|| {
            hyperliquid_adapter_validation_error(
                context.client_key,
                "execution.live_submit_product_proof_artifact_path",
                "Hyperliquid live submit requires configured product proof artifact path",
            )
        })?;
    let product_proof_artifact_sha256 = cfg
        .live_submit_product_proof_artifact_sha256
        .clone()
        .ok_or_else(|| {
            hyperliquid_adapter_validation_error(
                context.client_key,
                "execution.live_submit_product_proof_artifact_sha256",
                "Hyperliquid live submit requires configured product proof artifact sha256",
            )
        })?;
    Ok(HyperliquidLiveSubmitApprovalBinding {
        base_sha: context.build_head_sha.to_string(),
        provider_id: context.client_key.to_string(),
        product_surface: *product_surface,
        toml_checksum: context.loaded.config_bundle_checksum.clone(),
        signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(
            secrets.private_key.as_str(),
        ),
        order_limits: HyperliquidLiveSubmitOrderLimits {
            max_order_count,
            max_order_notional,
        },
        product_submit_proof: HyperliquidProductSubmitProofBinding {
            artifact_path: product_proof_artifact_path,
            artifact_sha256: product_proof_artifact_sha256,
        },
    })
}

fn validate_product_submit_proof_artifact(
    client_key: &str,
    root_path: &Path,
    binding: &HyperliquidLiveSubmitApprovalBinding,
    max_bytes: u64,
) -> Result<(), anyhow::Error> {
    let product_submit_proof = &binding.product_submit_proof;
    let resolved_path = resolve_root_relative_path(root_path, &product_submit_proof.artifact_path);
    let bytes = read_file_bounded(&resolved_path, max_bytes).map_err(|source| {
        hyperliquid_adapter_validation_error(
            client_key,
            "product_submit_proof.artifact_path",
            format!("Hyperliquid product submit proof artifact could not be read: {source}"),
        )
    })?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    if actual_sha256 != product_submit_proof.artifact_sha256 {
        return Err(hyperliquid_adapter_validation_error(
            client_key,
            "product_submit_proof.artifact_sha256",
            "Hyperliquid product submit proof artifact sha256 does not match configured live-submit approval binding",
        ));
    }
    validate_hyperliquid_product_submit_proof_artifact_bytes(&bytes, binding)
        .map_err(anyhow::Error::new)
}

fn hyperliquid_adapter_validation_error(
    client_key: &str,
    field: &'static str,
    message: impl Into<String>,
) -> anyhow::Error {
    anyhow::Error::new(BoltV3AdapterMappingError::ValidationInvariant {
        client_key: client_key.to_string(),
        field,
        message: message.into(),
    })
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

#[derive(Debug)]
struct HyperliquidUserFeesFeeProvider {
    http_client: HyperliquidHttpClient,
    account_address: String,
    account_fee_bps: Mutex<Option<Decimal>>,
}

impl FeeProvider for HyperliquidUserFeesFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        self.account_fee_bps
            .lock()
            .ok()
            .and_then(|fee_bps| *fee_bps)
    }

    fn warm(&self, instrument_id: InstrumentId) -> BoxFuture<'_, anyhow::Result<()>> {
        async move {
            if self.fee_bps(instrument_id).is_some() {
                return Ok(());
            }
            let response = self
                .http_client
                .info_user_fees(&self.account_address)
                .await
                .map_err(|source| {
                    anyhow::anyhow!("Hyperliquid userFees request failed: {source}")
                })?;
            let taker_fee_bps = hyperliquid_user_cross_fee_bps(&response)?;
            *self
                .account_fee_bps
                .lock()
                .map_err(|_| anyhow::anyhow!("Hyperliquid fee cache mutex poisoned"))? =
                Some(taker_fee_bps);
            Ok(())
        }
        .boxed()
    }
}

fn hyperliquid_user_cross_fee_bps(response: &serde_json::Value) -> anyhow::Result<Decimal> {
    let raw_rate = response
        .get(USER_CROSS_RATE_FIELD)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Hyperliquid userFees response missing userCrossRate"))?;
    let rate = Decimal::from_str(raw_rate.trim())
        .map_err(|source| anyhow::anyhow!("Hyperliquid userCrossRate parse failed: {source}"))?;
    if rate < Decimal::ZERO {
        return Err(anyhow::anyhow!(
            "Hyperliquid userCrossRate must be non-negative"
        ));
    }
    Ok(rate * Decimal::from(BASIS_POINTS_PER_UNIT_RATE))
}

fn hyperliquid_fee_http_client(
    client_key: &str,
    cfg: &HyperliquidExecutionConfig,
    secrets: &ResolvedBoltV3HyperliquidSecrets,
) -> Result<HyperliquidHttpClient, BoltV3AdapterMappingError> {
    let mut http_client = HyperliquidHttpClient::with_credentials(
        Some(secrets.private_key.as_str().to_owned()),
        secrets
            .vault_address
            .as_ref()
            .map(|vault_address| vault_address.as_str().to_owned()),
        Some(secrets.account_address.as_str().to_owned()),
        nt_environment(cfg.environment),
        cfg.http_timeout_secs,
        cfg.proxy_url.clone(),
    )
    .map_err(|source| BoltV3AdapterMappingError::ValidationInvariant {
        client_key: client_key.to_string(),
        field: "execution",
        message: format!("failed to create Hyperliquid fee HTTP client: {source}"),
    })?;
    http_client.set_base_info_url(cfg.base_url_http.clone());
    Ok(http_client)
}

pub fn build_fee_provider(
    client_key: &str,
    client: &ClientBlock,
    resolved: &crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<Arc<dyn FeeProvider>, BoltV3AdapterMappingError> {
    let value = client.execution.as_ref().ok_or_else(|| {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution",
            message: "is required by the Hyperliquid fee-provider boundary".to_string(),
        }
    })?;
    let cfg: HyperliquidExecutionConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                client_key: client_key.to_string(),
                block: "execution",
                message: error.to_string(),
            }
        })?;
    if let Some(message) = validate_execution_config(client_key, &cfg)
        .into_iter()
        .next()
    {
        return Err(BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "execution",
            message,
        });
    }
    let secrets = resolved
        .get_as::<ResolvedBoltV3HyperliquidSecrets>(client_key)
        .ok_or_else(|| BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "secrets",
            message: "resolved Hyperliquid secrets are required by the fee-provider boundary"
                .to_string(),
        })?;
    let http_client = hyperliquid_fee_http_client(client_key, &cfg, secrets)?;
    Ok(Arc::new(HyperliquidUserFeesFeeProvider {
        http_client,
        account_address: secrets.account_address.as_str().to_owned(),
        account_fee_bps: Mutex::new(None),
    }))
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.client.data {
        Some(value) => Some(BoltV3DataClientAdapterConfig {
            factory: Box::new(HyperliquidDataClientFactory),
            config: Box::new(map_data(context.client_key, value)?),
        }),
        None => None,
    };
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
    Ok(BoltV3ClientAdapterConfig { data, execution })
}

fn map_data(
    client_key: &str,
    value: &toml::Value,
) -> Result<HyperliquidDataClientConfig, BoltV3AdapterMappingError> {
    let cfg: HyperliquidDataConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                client_key: client_key.to_string(),
                block: "data",
                message: error.to_string(),
            }
        })?;
    let validation_errors = validate_data_config(client_key, &cfg);
    if let Some(message) = validation_errors.into_iter().next() {
        return Err(BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message,
        });
    }
    Ok(HyperliquidDataClientConfig {
        private_key: None,
        base_url_ws: Some(cfg.base_url_ws),
        base_url_http: Some(cfg.base_url_http),
        proxy_url: cfg.proxy_url,
        environment: nt_environment(cfg.environment),
        http_timeout_secs: cfg.http_timeout_secs,
        ws_timeout_secs: cfg.ws_timeout_secs,
        update_instruments_interval_mins: cfg.update_instruments_interval_mins,
        transport_backend: cfg.transport_backend,
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
    validate_target_surfaces(context.client_key, &cfg, context.plan)?;
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
            include_builder_attribution: cfg.include_builder_attribution,
            transport_backend: cfg.transport_backend,
            ws_post_timeout_secs: cfg.ws_post_timeout_secs,
            outcome_settlement_poll_secs: cfg.outcome_settlement_poll_secs,
        },
    })
}

fn validate_target_surfaces(
    client_key: &str,
    cfg: &HyperliquidExecutionConfig,
    plan: &MarketIdentityPlan,
) -> Result<(), BoltV3AdapterMappingError> {
    let [configured_surface] = cfg.product_surfaces.as_slice() else {
        return Ok(());
    };
    for target in hyperliquid_instrument::target_plans(plan)
        .filter(|target| target.execution_client_id == client_key)
    {
        let target_surface = hyperliquid_static_instrument_surface(target.product_surface);
        if target_surface != *configured_surface {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                client_key: client_key.to_string(),
                field: "strategy.target.product_surface",
                message: format!(
                    "configured target `{}` uses Hyperliquid product surface `{}` on client `{}`, but execution.product_surfaces selects `{}`",
                    target.configured_target_id,
                    hyperliquid_static_instrument_surface_name(target.product_surface),
                    target.execution_client_id,
                    product_surface_name(*configured_surface),
                ),
            });
        }
    }
    for target in
        updown::target_plans(plan).filter(|target| target.execution_client_id == client_key)
    {
        if *configured_surface != HyperliquidProductSurface::Hip4Outcomes {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                client_key: client_key.to_string(),
                field: "strategy.target.rotating_market_family",
                message: format!(
                    "configured target `{}` uses `{}` market family on client `{}`, but Hyperliquid `{}` targets require execution.product_surfaces `{}` and this client selects `{}`",
                    target.configured_target_id,
                    updown::KEY,
                    target.execution_client_id,
                    updown::KEY,
                    product_surface_name(HyperliquidProductSurface::Hip4Outcomes),
                    product_surface_name(*configured_surface),
                ),
            });
        }
    }
    for target in
        outcome_group::target_plans(plan).filter(|target| target.execution_client_id == client_key)
    {
        if *configured_surface != HyperliquidProductSurface::Hip4Outcomes {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                client_key: client_key.to_string(),
                field: "strategy.target.rotating_market_family",
                message: format!(
                    "configured target `{}` uses `{}` market family on client `{}`, but Hyperliquid outcome-group targets require execution.product_surfaces `{}` and this client selects `{}`",
                    target.configured_target_id,
                    outcome_group::KEY,
                    target.execution_client_id,
                    product_surface_name(HyperliquidProductSurface::Hip4Outcomes),
                    product_surface_name(*configured_surface),
                ),
            });
        }
    }
    Ok(())
}

fn validate_target_surfaces_for_live_submit_approval(
    context: &ProviderLiveSubmitApprovalContext<'_>,
    cfg: &HyperliquidExecutionConfig,
) -> Result<(), anyhow::Error> {
    let plan = market_identity_plan_from_config(context.loaded).map_err(|source| {
        hyperliquid_adapter_validation_error(
            context.client_key,
            "strategy.target",
            format!("Hyperliquid target routing is invalid: {source}"),
        )
    })?;
    validate_target_surfaces(context.client_key, cfg, &plan).map_err(anyhow::Error::new)
}

fn hyperliquid_static_instrument_surface(
    surface: hyperliquid_instrument::ProductSurface,
) -> HyperliquidProductSurface {
    match surface {
        hyperliquid_instrument::ProductSurface::StandardPerps => {
            HyperliquidProductSurface::StandardPerps
        }
        hyperliquid_instrument::ProductSurface::Spot => HyperliquidProductSurface::Spot,
        hyperliquid_instrument::ProductSurface::Hip3BuilderPerps => {
            HyperliquidProductSurface::Hip3BuilderPerps
        }
        hyperliquid_instrument::ProductSurface::Hip4Outcomes => {
            HyperliquidProductSurface::Hip4Outcomes
        }
    }
}

fn hyperliquid_static_instrument_surface_name(
    surface: hyperliquid_instrument::ProductSurface,
) -> &'static str {
    product_surface_name(hyperliquid_static_instrument_surface(surface))
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
    let Some(consumed_payload) = context.runtime_approvals.live_submit else {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution.live_submit_approval_id",
            message: format!(
                "{} live submit requires a consumed live-submit approval",
                product_surface_name(*configured_surface)
            ),
        });
    };
    let consumed = if let Some(consumed) =
        consumed_payload.downcast_ref::<HyperliquidLiveSubmitApprovalConsumption>()
    {
        consumed
    } else if let Some(approvals) = consumed_payload.downcast_ref::<ProviderLiveSubmitApprovals>() {
        approvals
            .get_as::<HyperliquidLiveSubmitApprovalConsumption>(client_key)
            .ok_or_else(|| BoltV3AdapterMappingError::ValidationInvariant {
                client_key: client_key.to_string(),
                field: "execution.live_submit_approval_id",
                message: "consumed live-submit approval bundle does not contain this client"
                    .to_string(),
            })?
    } else {
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

fn parse_product_surface_name(value: &str) -> Option<HyperliquidProductSurface> {
    [
        HyperliquidProductSurface::StandardPerps,
        HyperliquidProductSurface::Spot,
        HyperliquidProductSurface::Hip3BuilderPerps,
        HyperliquidProductSurface::Hip4Outcomes,
    ]
    .into_iter()
    .find(|surface| product_surface_name(*surface) == value)
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
