use std::fmt;

use serde::Deserialize;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::bolt_v3_providers::SsmSecretResolver;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RedemptionConfig {
    pub schema_version: u32,
    pub enabled: bool,
    pub provider_manifest_id: String,
    pub wallet: WalletConfig,
    pub adapter: AdapterConfig,
    pub relayer: RelayerConfig,
    pub rpc: RpcConfig,
    pub credentials: CredentialPaths,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WalletConfig {
    pub chain_id: u64,
    pub wallet_type: String,
    pub safe_address: String,
    pub safe_factory: String,
    pub safe_implementation: String,
    pub fallback_handler: String,
    pub guard: String,
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterConfig {
    pub standard_target: String,
    pub negative_risk_target: String,
    pub collateral: String,
    pub output_asset: String,
    pub dummy_account: String,
    pub dummy_parent_collection_id: String,
    pub dummy_index_sets: Vec<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RelayerConfig {
    pub origin: String,
    pub submit_path: String,
    pub transaction_path: String,
    pub nonce_path: String,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub max_transaction_items: usize,
    pub max_transaction_id_bytes: usize,
    pub max_metadata_bytes: usize,
    pub competing_same_nonce_conformance: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RpcConfig {
    pub max_response_bytes: usize,
    pub max_receipt_logs: usize,
    pub finality_confirmations: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialPaths {
    pub signer_private_key_ssm_path: String,
    pub builder_api_key_ssm_path: String,
    pub builder_api_secret_ssm_path: String,
    pub builder_passphrase_ssm_path: String,
    pub max_value_bytes: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderManifest {
    pub schema_version: u32,
    pub manifest_id: String,
    pub adapter: ManifestAdapter,
    pub adapter_arguments: ManifestAdapterArguments,
    pub deployment: ManifestDeployment,
    pub safe_boundary: ManifestSafeBoundary,
    pub safe: ManifestSafe,
    pub relayer: ManifestRelayer,
    pub independent_fixture: ManifestIndependentFixture,
    pub source_snapshots: ManifestSourceSnapshots,
    pub activation: ManifestActivation,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestAdapter {
    pub reviewed_repository: String,
    pub reviewed_revision: String,
    pub standard_source: String,
    pub standard_blob: String,
    pub negative_risk_source: String,
    pub negative_risk_blob: String,
    pub negative_risk_interface_source: String,
    pub negative_risk_interface_blob: String,
    pub deployment_table_blob: String,
    pub external_abi: String,
    pub external_selector: String,
    pub ignored_argument_indices: Vec<usize>,
    pub standard_internal_path: String,
    pub negative_risk_internal_path: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestAdapterArguments {
    pub dummy_account: String,
    pub dummy_parent_collection_id: String,
    pub dummy_index_sets: Vec<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestDeployment {
    pub chain_id: u64,
    pub wallet_type: String,
    pub safe_address: String,
    pub safe_factory: String,
    pub standard_target: String,
    pub negative_risk_target: String,
    pub collateral: String,
    pub output_asset: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestSafeBoundary {
    pub verification: String,
    pub implementation: String,
    pub fallback_handler: String,
    pub guard: String,
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestSafe {
    pub nonce_abi: String,
    pub nonce_selector: String,
    pub operation: String,
    pub value: String,
    pub safe_tx_gas: String,
    pub base_gas: String,
    pub gas_price: String,
    pub gas_token: String,
    pub refund_receiver: String,
    pub signature_bytes: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestRelayer {
    pub reviewed_repository: String,
    pub reviewed_revision: String,
    pub safe_builder_blob: String,
    pub types_blob: String,
    pub client_blob: String,
    pub endpoints_blob: String,
    pub safe_abi_blob: String,
    pub config_blob: String,
    pub explicit_nonce: String,
    pub competing_same_nonce: String,
    pub submit_path: String,
    pub transaction_path: String,
    pub nonce_path: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestIndependentFixture {
    pub reviewed_repository: String,
    pub reviewed_revision: String,
    pub safe_builder_blob: String,
    pub client_blob: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestSourceSnapshots {
    pub standard_sha256: String,
    pub negative_risk_sha256: String,
    pub relayer_sha256: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestActivation {
    pub primitive_enabled: bool,
    pub requires_competing_same_nonce_conformance: bool,
    pub has_active_caller: bool,
    pub has_durable_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedemptionConfigError(pub String);

impl fmt::Display for RedemptionConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RedemptionConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRedemptionProfile {
    pub config: RedemptionConfig,
    pub manifest: ProviderManifest,
}

pub fn validate_profile(
    config_toml: &str,
    manifest_toml: &str,
) -> Result<ValidatedRedemptionProfile, RedemptionConfigError> {
    let config: RedemptionConfig = toml::from_str(config_toml)
        .map_err(|error| RedemptionConfigError(format!("invalid redemption TOML: {error}")))?;
    let manifest: ProviderManifest = toml::from_str(manifest_toml)
        .map_err(|error| RedemptionConfigError(format!("invalid provider manifest: {error}")))?;
    if config.enabled || manifest.activation.primitive_enabled {
        return Err(RedemptionConfigError(
            "AO-REDEEM must remain mechanically disabled".to_string(),
        ));
    }
    if config.schema_version != manifest.schema_version
        || config.provider_manifest_id != manifest.manifest_id
    {
        return Err(RedemptionConfigError(
            "redemption TOML does not match the source-fenced manifest".to_string(),
        ));
    }
    if config.wallet.chain_id != manifest.deployment.chain_id
        || config.wallet.wallet_type != manifest.deployment.wallet_type
        || config.wallet.wallet_type != "SAFE"
    {
        return Err(RedemptionConfigError(
            "redemption wallet binding does not match the source-fenced deployment".to_string(),
        ));
    }
    if manifest.adapter.external_abi != "redeemPositions(address,bytes32,bytes32,uint256[])"
        || manifest.adapter.ignored_argument_indices != [0, 1, 3]
        || manifest.safe.nonce_abi != "nonce()"
        || manifest.safe.operation != "call"
        || manifest.safe.value != "0"
        || manifest.safe.safe_tx_gas != "0"
        || manifest.safe.base_gas != "0"
        || manifest.safe.gas_price != "0"
        || !is_zero_address(&manifest.safe.gas_token)
        || !is_zero_address(&manifest.safe.refund_receiver)
        || manifest.safe.signature_bytes != 65
        || manifest.safe_boundary.verification != "exact-query-required"
    {
        return Err(RedemptionConfigError(
            "provider manifest ABI or Safe fence semantics drifted".to_string(),
        ));
    }
    for (configured, fenced) in [
        (
            &config.wallet.safe_address,
            &manifest.deployment.safe_address,
        ),
        (
            &config.wallet.safe_factory,
            &manifest.deployment.safe_factory,
        ),
        (
            &config.adapter.standard_target,
            &manifest.deployment.standard_target,
        ),
        (
            &config.adapter.negative_risk_target,
            &manifest.deployment.negative_risk_target,
        ),
        (&config.adapter.collateral, &manifest.deployment.collateral),
        (
            &config.adapter.output_asset,
            &manifest.deployment.output_asset,
        ),
        (
            &config.adapter.dummy_account,
            &manifest.adapter_arguments.dummy_account,
        ),
        (
            &config.adapter.dummy_parent_collection_id,
            &manifest.adapter_arguments.dummy_parent_collection_id,
        ),
        (
            &config.wallet.safe_implementation,
            &manifest.safe_boundary.implementation,
        ),
        (
            &config.wallet.fallback_handler,
            &manifest.safe_boundary.fallback_handler,
        ),
        (&config.wallet.guard, &manifest.safe_boundary.guard),
    ] {
        if !configured.eq_ignore_ascii_case(fenced) {
            return Err(RedemptionConfigError(
                "redemption deployment binding does not match provider manifest".to_string(),
            ));
        }
    }
    if config.adapter.dummy_index_sets != manifest.adapter_arguments.dummy_index_sets
        || !address_lists_equal(&config.wallet.modules, &manifest.safe_boundary.modules)
    {
        return Err(RedemptionConfigError(
            "redemption adapter arguments or Safe modules drifted from provider manifest"
                .to_string(),
        ));
    }
    for address in [
        &config.wallet.safe_address,
        &config.wallet.safe_factory,
        &config.wallet.safe_implementation,
        &config.wallet.fallback_handler,
        &config.adapter.standard_target,
        &config.adapter.negative_risk_target,
        &config.adapter.collateral,
        &config.adapter.output_asset,
    ] {
        if !is_nonzero_address(address) {
            return Err(RedemptionConfigError(
                "redemption deployment contains an invalid nonzero address".to_string(),
            ));
        }
    }
    if !is_zero_address(&config.wallet.guard)
        || config
            .wallet
            .modules
            .iter()
            .any(|address| !is_nonzero_address(address))
        || !is_zero_address(&config.adapter.dummy_account)
        || !is_zero_bytes32(&config.adapter.dummy_parent_collection_id)
    {
        return Err(RedemptionConfigError(
            "redemption Safe boundary or ignored adapter arguments are invalid".to_string(),
        ));
    }
    if manifest.relayer.explicit_nonce != "source-proven"
        || manifest.relayer.competing_same_nonce != "unproven"
        || config.relayer.competing_same_nonce_conformance
    {
        return Err(RedemptionConfigError(
            "competing-same-nonce support is not conformantly proven".to_string(),
        ));
    }
    for (configured, fenced) in [
        (&config.relayer.submit_path, &manifest.relayer.submit_path),
        (
            &config.relayer.transaction_path,
            &manifest.relayer.transaction_path,
        ),
        (&config.relayer.nonce_path, &manifest.relayer.nonce_path),
    ] {
        if configured != fenced {
            return Err(RedemptionConfigError(
                "relayer route does not match provider manifest".to_string(),
            ));
        }
    }
    if !config.relayer.origin.starts_with("https://")
        || config.relayer.submit_path != "/submit"
        || config.relayer.transaction_path != "/transaction"
        || config.relayer.nonce_path != "/nonce"
    {
        return Err(RedemptionConfigError(
            "redemption relayer origin or exact query routes are invalid".to_string(),
        ));
    }
    if config.relayer.max_request_bytes == 0
        || config.relayer.max_response_bytes == 0
        || config.relayer.max_transaction_items != 1
        || config.relayer.max_transaction_id_bytes == 0
        || config.credentials.max_value_bytes == 0
        || config.rpc.max_response_bytes == 0
        || config.rpc.max_receipt_logs == 0
        || config.rpc.finality_confirmations == 0
    {
        return Err(RedemptionConfigError(
            "redemption wire and finality bounds must be positive and exact-id queries allow one item"
                .to_string(),
        ));
    }
    for path in [
        &config.credentials.signer_private_key_ssm_path,
        &config.credentials.builder_api_key_ssm_path,
        &config.credentials.builder_api_secret_ssm_path,
        &config.credentials.builder_passphrase_ssm_path,
    ] {
        if !path.starts_with("/bolt/") || path.chars().any(char::is_whitespace) {
            return Err(RedemptionConfigError(
                "redemption credentials must be grouped SSM parameter paths".to_string(),
            ));
        }
    }
    Ok(ValidatedRedemptionProfile { config, manifest })
}

fn is_hex(value: &str, digits: usize) -> bool {
    value
        .strip_prefix("0x")
        .is_some_and(|hex| hex.len() == digits && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn is_zero_address(value: &str) -> bool {
    is_hex(value, 40) && value[2..].bytes().all(|byte| byte == b'0')
}

fn is_nonzero_address(value: &str) -> bool {
    is_hex(value, 40) && !is_zero_address(value)
}

fn is_zero_bytes32(value: &str) -> bool {
    is_hex(value, 64) && value[2..].bytes().all(|byte| byte == b'0')
}

fn address_lists_equal(configured: &[String], fenced: &[String]) -> bool {
    configured.len() == fenced.len()
        && configured
            .iter()
            .zip(fenced)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ResolvedRedemptionCredentials {
    signer_private_key: Zeroizing<String>,
    builder_api_key: Zeroizing<String>,
    builder_api_secret: Zeroizing<String>,
    builder_passphrase: Zeroizing<String>,
}

impl fmt::Debug for ResolvedRedemptionCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedRedemptionCredentials")
            .field("signer_private_key", &"<redacted>")
            .field("builder_api_key", &"<redacted>")
            .field("builder_api_secret", &"<redacted>")
            .field("builder_passphrase", &"<redacted>")
            .finish()
    }
}

impl ResolvedRedemptionCredentials {
    pub fn redaction_values(&self) -> [&str; 4] {
        [
            self.signer_private_key.as_str(),
            self.builder_api_key.as_str(),
            self.builder_api_secret.as_str(),
            self.builder_passphrase.as_str(),
        ]
    }
}

pub fn resolve_credentials(
    profile: &ValidatedRedemptionProfile,
    region: &str,
    resolver: &mut dyn SsmSecretResolver,
) -> Result<ResolvedRedemptionCredentials, RedemptionConfigError> {
    fn one(
        region: &str,
        path: &str,
        max_bytes: usize,
        resolver: &mut dyn SsmSecretResolver,
    ) -> Result<Zeroizing<String>, RedemptionConfigError> {
        let value = resolver
            .resolve_secret(region, path)
            .map_err(|_| RedemptionConfigError("SSM credential resolution failed".to_string()))?;
        if value.is_empty() || value.len() > max_bytes {
            return Err(RedemptionConfigError(
                "SSM credential value violates the configured bound".to_string(),
            ));
        }
        Ok(Zeroizing::new(value))
    }

    let paths = &profile.config.credentials;
    Ok(ResolvedRedemptionCredentials {
        signer_private_key: one(
            region,
            &paths.signer_private_key_ssm_path,
            paths.max_value_bytes,
            resolver,
        )?,
        builder_api_key: one(
            region,
            &paths.builder_api_key_ssm_path,
            paths.max_value_bytes,
            resolver,
        )?,
        builder_api_secret: one(
            region,
            &paths.builder_api_secret_ssm_path,
            paths.max_value_bytes,
            resolver,
        )?,
        builder_passphrase: one(
            region,
            &paths.builder_passphrase_ssm_path,
            paths.max_value_bytes,
            resolver,
        )?,
    })
}
