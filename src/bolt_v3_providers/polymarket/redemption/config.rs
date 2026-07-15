use std::fmt;

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use super::bounded::{ProjectionClass, RedactedProjection};

const ADDRESS_BYTES: usize = 20;
const WORD_BYTES: usize = 32;
const SELECTOR_BYTES: usize = 4;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    schema_version: u32,
    enabled: bool,
    provider_manifest_id: String,
    wallet: RawWallet,
    adapter: RawAdapter,
    relayer: RawRelayer,
    rpc: RawRpc,
    query: RawQuery,
    credentials: RawCredentialPaths,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWallet {
    chain_id: u64,
    wallet_type: String,
    safe_address: String,
    safe_factory: String,
    safe_implementation: String,
    fallback_handler: String,
    guard: String,
    modules: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAdapter {
    standard_target: String,
    negative_risk_target: String,
    collateral: String,
    output_asset: String,
    dummy_account: String,
    dummy_parent_collection_id: String,
    dummy_index_sets: Vec<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRelayer {
    origin: String,
    submit_path: String,
    transaction_path: String,
    nonce_path: String,
    max_origin_bytes: usize,
    max_path_bytes: usize,
    max_request_bytes: usize,
    max_response_bytes: usize,
    overflow_probe_bytes: usize,
    max_transaction_items: usize,
    max_transaction_id_bytes: usize,
    max_timestamp_bytes: usize,
    max_metadata_bytes: usize,
    max_header_bytes: usize,
    competing_same_nonce_conformance: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRpc {
    max_response_bytes: usize,
    overflow_probe_bytes: usize,
    max_receipt_logs: usize,
    finality_confirmations: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawQuery {
    max_items: usize,
    max_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCredentialPaths {
    signer_private_key_ssm_path: String,
    builder_api_key_ssm_path: String,
    builder_api_secret_ssm_path: String,
    builder_passphrase_ssm_path: String,
    redaction_hmac_key_ssm_path: String,
    max_value_bytes: usize,
    max_acquisition_bytes: usize,
    max_path_bytes: usize,
    key_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    schema_version: u32,
    manifest_id: String,
    adapter: RawManifestAdapter,
    adapter_arguments: RawManifestAdapterArguments,
    deployment: RawManifestDeployment,
    safe_boundary: RawManifestSafeBoundary,
    safe: RawManifestSafe,
    relayer: RawManifestRelayer,
    independent_fixture: RawManifestIndependentFixture,
    source_snapshots: RawManifestSourceSnapshots,
    wire: RawManifestWire,
    credential_boundary: RawManifestCredentialBoundary,
    activation: RawManifestActivation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestAdapter {
    reviewed_repository: String,
    reviewed_revision: String,
    standard_source: String,
    standard_blob: String,
    negative_risk_source: String,
    negative_risk_blob: String,
    negative_risk_interface_source: String,
    negative_risk_interface_blob: String,
    deployment_table_blob: String,
    external_abi: String,
    external_selector: String,
    ignored_argument_indices: Vec<usize>,
    standard_internal_path: String,
    negative_risk_internal_path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestAdapterArguments {
    dummy_account: String,
    dummy_parent_collection_id: String,
    dummy_index_sets: Vec<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestDeployment {
    chain_id: u64,
    wallet_type: String,
    safe_address: String,
    safe_factory: String,
    standard_target: String,
    negative_risk_target: String,
    collateral: String,
    output_asset: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestSafeBoundary {
    verification: String,
    implementation: String,
    fallback_handler: String,
    guard: String,
    modules: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestSafe {
    nonce_abi: String,
    nonce_selector: String,
    operation: String,
    value: String,
    safe_tx_gas: String,
    base_gas: String,
    gas_price: String,
    gas_token: String,
    refund_receiver: String,
    signature_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestRelayer {
    reviewed_repository: String,
    reviewed_revision: String,
    safe_builder_blob: String,
    types_blob: String,
    client_blob: String,
    endpoints_blob: String,
    safe_abi_blob: String,
    config_blob: String,
    response_blob: String,
    state_schema_blob: String,
    signing_repository: String,
    signing_revision: String,
    signing_hmac_blob: String,
    explicit_nonce: String,
    competing_same_nonce: String,
    submit_path: String,
    transaction_path: String,
    nonce_path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestIndependentFixture {
    reviewed_repository: String,
    reviewed_revision: String,
    safe_builder_blob: String,
    client_blob: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestSourceSnapshots {
    standard_sha256: String,
    negative_risk_sha256: String,
    relayer_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestWire {
    max_query_items: usize,
    max_query_bytes: usize,
    max_transaction_items: usize,
    max_header_items: usize,
    max_execution_logs: usize,
    overflow_probe_bytes: usize,
    receipt_schema_repository: String,
    receipt_schema_revision: String,
    receipt_schema_blob: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestCredentialBoundary {
    source: String,
    max_acquisition_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifestActivation {
    primitive_enabled: bool,
    requires_competing_same_nonce_conformance: bool,
    has_active_caller: bool,
    has_durable_state: bool,
}

pub struct ValidatedRedemptionProfile {
    chain_id: u64,
    safe_address: [u8; ADDRESS_BYTES],
    safe_factory: [u8; ADDRESS_BYTES],
    safe_implementation: [u8; ADDRESS_BYTES],
    fallback_handler: [u8; ADDRESS_BYTES],
    guard: [u8; ADDRESS_BYTES],
    standard_target: [u8; ADDRESS_BYTES],
    negative_risk_target: [u8; ADDRESS_BYTES],
    collateral: [u8; ADDRESS_BYTES],
    output_asset: [u8; ADDRESS_BYTES],
    dummy_account: [u8; ADDRESS_BYTES],
    dummy_parent: [u8; WORD_BYTES],
    dummy_index_sets: [u64; 2],
    redemption_selector: [u8; SELECTOR_BYTES],
    nonce_selector: [u8; SELECTOR_BYTES],
    relayer_origin: Box<str>,
    submit_path: Box<str>,
    transaction_path: Box<str>,
    nonce_path: Box<str>,
    max_request_bytes: usize,
    max_relayer_response_bytes: usize,
    max_rpc_response_bytes: usize,
    overflow_probe_bytes: usize,
    max_transaction_id_bytes: usize,
    max_timestamp_bytes: usize,
    max_metadata_bytes: usize,
    max_header_bytes: usize,
    max_receipt_logs: usize,
    finality_confirmations: u64,
    max_query_items: usize,
    max_query_bytes: usize,
    credential_paths: [Box<str>; 5],
    max_credential_bytes: usize,
    max_credential_acquisition_bytes: usize,
    key_version: u32,
}

impl ValidatedRedemptionProfile {
    pub fn max_request_bytes(&self) -> usize {
        self.max_request_bytes
    }

    pub fn max_relayer_response_bytes(&self) -> usize {
        self.max_relayer_response_bytes
    }

    pub fn max_rpc_response_bytes(&self) -> usize {
        self.max_rpc_response_bytes
    }

    pub fn max_metadata_bytes(&self) -> usize {
        self.max_metadata_bytes
    }

    pub fn max_query_items(&self) -> usize {
        self.max_query_items
    }

    pub fn max_query_bytes(&self) -> usize {
        self.max_query_bytes
    }

    pub(super) fn chain_id(&self) -> u64 {
        self.chain_id
    }

    pub(super) fn safe_address(&self) -> [u8; ADDRESS_BYTES] {
        self.safe_address
    }

    pub(super) fn target(&self, negative_risk: bool) -> [u8; ADDRESS_BYTES] {
        if negative_risk {
            self.negative_risk_target
        } else {
            self.standard_target
        }
    }

    pub(super) fn redemption_selector(&self) -> [u8; SELECTOR_BYTES] {
        self.redemption_selector
    }

    pub(super) fn nonce_selector(&self) -> [u8; SELECTOR_BYTES] {
        self.nonce_selector
    }

    pub(super) fn adapter_arguments(&self) -> ([u8; ADDRESS_BYTES], [u8; WORD_BYTES], [u64; 2]) {
        (self.dummy_account, self.dummy_parent, self.dummy_index_sets)
    }

    pub(super) fn overflow_probe_bytes(&self) -> usize {
        self.overflow_probe_bytes
    }

    pub(super) fn max_transaction_id_bytes(&self) -> usize {
        self.max_transaction_id_bytes
    }

    pub(super) fn max_timestamp_bytes(&self) -> usize {
        self.max_timestamp_bytes
    }

    pub(super) fn max_receipt_logs(&self) -> usize {
        self.max_receipt_logs
    }

    pub(super) fn finality_confirmations(&self) -> u64 {
        self.finality_confirmations
    }

    pub(super) fn transaction_path(&self) -> &str {
        &self.transaction_path
    }

    pub(super) fn submit_path(&self) -> &str {
        &self.submit_path
    }

    pub(super) fn safe_boundary(
        &self,
    ) -> (
        [u8; ADDRESS_BYTES],
        [u8; ADDRESS_BYTES],
        [u8; ADDRESS_BYTES],
        [u8; ADDRESS_BYTES],
    ) {
        (
            self.safe_factory,
            self.safe_implementation,
            self.fallback_handler,
            self.guard,
        )
    }

    pub(super) fn max_header_bytes(&self) -> usize {
        self.max_header_bytes
    }
}

impl Drop for ValidatedRedemptionProfile {
    fn drop(&mut self) {
        self.chain_id.zeroize();
        self.safe_address.zeroize();
        self.safe_factory.zeroize();
        self.safe_implementation.zeroize();
        self.fallback_handler.zeroize();
        self.guard.zeroize();
        self.standard_target.zeroize();
        self.negative_risk_target.zeroize();
        self.collateral.zeroize();
        self.output_asset.zeroize();
        self.dummy_account.zeroize();
        self.dummy_parent.zeroize();
        self.dummy_index_sets.zeroize();
        self.redemption_selector.zeroize();
        self.nonce_selector.zeroize();
        self.relayer_origin.as_mut().zeroize();
        self.submit_path.as_mut().zeroize();
        self.transaction_path.as_mut().zeroize();
        self.nonce_path.as_mut().zeroize();
        for path in &mut self.credential_paths {
            path.as_mut().zeroize();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedemptionConfigError {
    InvalidToml,
    Enabled,
    ManifestMismatch,
    ManifestDrift,
    DeploymentMismatch,
    InvalidAddress,
    InvalidBounds,
    InvalidSsmPath,
    SecretResolution,
    SecretBound,
}

impl fmt::Display for RedemptionConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "redacted redemption configuration failure: {self:?}"
        )
    }
}

impl std::error::Error for RedemptionConfigError {}

pub fn validate_profile(
    config_toml: &str,
    manifest_toml: &str,
) -> Result<ValidatedRedemptionProfile, RedemptionConfigError> {
    let config: RawConfig =
        toml::from_str(config_toml).map_err(|_| RedemptionConfigError::InvalidToml)?;
    let manifest: RawManifest =
        toml::from_str(manifest_toml).map_err(|_| RedemptionConfigError::InvalidToml)?;
    if config.enabled || manifest.activation.primitive_enabled {
        return Err(RedemptionConfigError::Enabled);
    }
    if config.schema_version != manifest.schema_version
        || config.provider_manifest_id != manifest.manifest_id
    {
        return Err(RedemptionConfigError::ManifestMismatch);
    }
    validate_manifest(&manifest, &config)?;
    let deployment = &manifest.deployment;
    for (configured, fenced) in [
        (&config.wallet.safe_address, &deployment.safe_address),
        (&config.wallet.safe_factory, &deployment.safe_factory),
        (
            &config.wallet.safe_implementation,
            &manifest.safe_boundary.implementation,
        ),
        (
            &config.wallet.fallback_handler,
            &manifest.safe_boundary.fallback_handler,
        ),
        (&config.wallet.guard, &manifest.safe_boundary.guard),
        (&config.adapter.standard_target, &deployment.standard_target),
        (
            &config.adapter.negative_risk_target,
            &deployment.negative_risk_target,
        ),
        (&config.adapter.collateral, &deployment.collateral),
        (&config.adapter.output_asset, &deployment.output_asset),
        (
            &config.adapter.dummy_account,
            &manifest.adapter_arguments.dummy_account,
        ),
        (
            &config.adapter.dummy_parent_collection_id,
            &manifest.adapter_arguments.dummy_parent_collection_id,
        ),
    ] {
        if !configured.eq_ignore_ascii_case(fenced) {
            return Err(RedemptionConfigError::DeploymentMismatch);
        }
    }
    if config.wallet.modules != manifest.safe_boundary.modules
        || !config.wallet.modules.is_empty()
        || config.adapter.dummy_index_sets != manifest.adapter_arguments.dummy_index_sets
        || config.adapter.dummy_index_sets.as_slice() != [1, 2]
    {
        return Err(RedemptionConfigError::DeploymentMismatch);
    }
    let credential_paths = [
        config.credentials.signer_private_key_ssm_path,
        config.credentials.builder_api_key_ssm_path,
        config.credentials.builder_api_secret_ssm_path,
        config.credentials.builder_passphrase_ssm_path,
        config.credentials.redaction_hmac_key_ssm_path,
    ];
    for path in &credential_paths {
        if !path.starts_with("/bolt/")
            || path.len() > config.credentials.max_path_bytes
            || path.chars().any(char::is_whitespace)
        {
            return Err(RedemptionConfigError::InvalidSsmPath);
        }
    }
    let profile = ValidatedRedemptionProfile {
        chain_id: config.wallet.chain_id,
        safe_address: parse_address(&config.wallet.safe_address, false)?,
        safe_factory: parse_address(&config.wallet.safe_factory, false)?,
        safe_implementation: parse_address(&config.wallet.safe_implementation, false)?,
        fallback_handler: parse_address(&config.wallet.fallback_handler, false)?,
        guard: parse_address(&config.wallet.guard, true)?,
        standard_target: parse_address(&config.adapter.standard_target, false)?,
        negative_risk_target: parse_address(&config.adapter.negative_risk_target, false)?,
        collateral: parse_address(&config.adapter.collateral, false)?,
        output_asset: parse_address(&config.adapter.output_asset, false)?,
        dummy_account: parse_address(&config.adapter.dummy_account, true)?,
        dummy_parent: parse_fixed(&config.adapter.dummy_parent_collection_id)?,
        dummy_index_sets: [1, 2],
        redemption_selector: parse_fixed(&manifest.adapter.external_selector)?,
        nonce_selector: parse_fixed(&manifest.safe.nonce_selector)?,
        relayer_origin: config.relayer.origin.into_boxed_str(),
        submit_path: config.relayer.submit_path.into_boxed_str(),
        transaction_path: config.relayer.transaction_path.into_boxed_str(),
        nonce_path: config.relayer.nonce_path.into_boxed_str(),
        max_request_bytes: config.relayer.max_request_bytes,
        max_relayer_response_bytes: config.relayer.max_response_bytes,
        max_rpc_response_bytes: config.rpc.max_response_bytes,
        overflow_probe_bytes: config.relayer.overflow_probe_bytes,
        max_transaction_id_bytes: config.relayer.max_transaction_id_bytes,
        max_timestamp_bytes: config.relayer.max_timestamp_bytes,
        max_metadata_bytes: config.relayer.max_metadata_bytes,
        max_header_bytes: config.relayer.max_header_bytes,
        max_receipt_logs: config.rpc.max_receipt_logs,
        finality_confirmations: config.rpc.finality_confirmations,
        max_query_items: config.query.max_items,
        max_query_bytes: config.query.max_bytes,
        credential_paths: credential_paths.map(String::into_boxed_str),
        max_credential_bytes: config.credentials.max_value_bytes,
        max_credential_acquisition_bytes: config.credentials.max_acquisition_bytes,
        key_version: config.credentials.key_version,
    };
    Ok(profile)
}

fn validate_manifest(
    manifest: &RawManifest,
    config: &RawConfig,
) -> Result<(), RedemptionConfigError> {
    let adapter = &manifest.adapter;
    let safe = &manifest.safe;
    let relayer = &manifest.relayer;
    let wire = &manifest.wire;
    if adapter.reviewed_repository.is_empty()
        || adapter.reviewed_revision.len() != 40
        || adapter.standard_source.is_empty()
        || adapter.standard_blob.len() != 40
        || adapter.negative_risk_source.is_empty()
        || adapter.negative_risk_blob.len() != 40
        || adapter.negative_risk_interface_source.is_empty()
        || adapter.negative_risk_interface_blob.len() != 40
        || adapter.deployment_table_blob.len() != 40
        || adapter.external_abi != "redeemPositions(address,bytes32,bytes32,uint256[])"
        || adapter.ignored_argument_indices != [0, 1, 3]
        || adapter.standard_internal_path.is_empty()
        || adapter.negative_risk_internal_path.is_empty()
        || manifest.deployment.chain_id != config.wallet.chain_id
        || manifest.deployment.wallet_type != "SAFE"
        || config.wallet.wallet_type != "SAFE"
        || manifest.safe_boundary.verification != "exact-query-required"
        || safe.nonce_abi != "nonce()"
        || safe.operation != "call"
        || safe.value != "0"
        || safe.safe_tx_gas != "0"
        || safe.base_gas != "0"
        || safe.gas_price != "0"
        || !is_zero_address(&safe.gas_token)
        || !is_zero_address(&safe.refund_receiver)
        || safe.signature_bytes != 65
        || relayer.reviewed_repository.is_empty()
        || relayer.reviewed_revision.len() != 40
        || relayer.safe_builder_blob.len() != 40
        || relayer.types_blob.len() != 40
        || relayer.client_blob.len() != 40
        || relayer.endpoints_blob.len() != 40
        || relayer.safe_abi_blob.len() != 40
        || relayer.config_blob.len() != 40
        || relayer.response_blob.len() != 40
        || relayer.state_schema_blob != relayer.types_blob
        || relayer.signing_repository.is_empty()
        || relayer.signing_revision.len() != 40
        || relayer.signing_hmac_blob.len() != 40
        || relayer.explicit_nonce != "source-proven"
        || relayer.competing_same_nonce != "unproven"
        || config.relayer.competing_same_nonce_conformance
        || config.relayer.submit_path != relayer.submit_path
        || config.relayer.transaction_path != relayer.transaction_path
        || config.relayer.nonce_path != relayer.nonce_path
        || config.relayer.origin.len() > config.relayer.max_origin_bytes
        || config.relayer.submit_path.len() > config.relayer.max_path_bytes
        || config.relayer.transaction_path.len() > config.relayer.max_path_bytes
        || config.relayer.nonce_path.len() > config.relayer.max_path_bytes
        || !config.relayer.origin.starts_with("https://")
        || wire.max_query_items == 0
        || wire.max_transaction_items != 1
        || wire.max_header_items != 4
        || wire.max_execution_logs != 1
        || wire.overflow_probe_bytes != 1
        || wire.receipt_schema_repository.is_empty()
        || wire.receipt_schema_revision.len() != 40
        || wire.receipt_schema_blob.len() != 40
        || config.relayer.max_transaction_items != wire.max_transaction_items
        || config.relayer.overflow_probe_bytes != wire.overflow_probe_bytes
        || config.rpc.overflow_probe_bytes != wire.overflow_probe_bytes
        || config.rpc.max_receipt_logs != wire.max_execution_logs
        || config.query.max_items == 0
        || config.query.max_items > wire.max_query_items
        || config.query.max_bytes == 0
        || config.query.max_bytes > wire.max_query_bytes
        || config.relayer.max_request_bytes == 0
        || config.relayer.max_response_bytes == 0
        || config.relayer.max_transaction_id_bytes == 0
        || config.relayer.max_timestamp_bytes == 0
        || config.relayer.max_origin_bytes == 0
        || config.relayer.max_path_bytes == 0
        || config.relayer.max_metadata_bytes == 0
        || config.relayer.max_header_bytes == 0
        || config.rpc.max_response_bytes == 0
        || config.rpc.max_receipt_logs == 0
        || config.rpc.finality_confirmations == 0
        || config.credentials.max_value_bytes == 0
        || config.credentials.max_acquisition_bytes < config.credentials.max_value_bytes
        || manifest.credential_boundary.source != "aws-ssm-capped-sink"
        || manifest.credential_boundary.max_acquisition_bytes == 0
        || config.credentials.max_acquisition_bytes
            > manifest.credential_boundary.max_acquisition_bytes
        || config.credentials.max_path_bytes == 0
        || config.credentials.key_version == 0
        || manifest.independent_fixture.reviewed_repository.is_empty()
        || manifest.independent_fixture.reviewed_revision.len() != 40
        || manifest.independent_fixture.safe_builder_blob.len() != 40
        || manifest.independent_fixture.client_blob.len() != 40
        || manifest.source_snapshots.standard_sha256.len() != 64
        || manifest.source_snapshots.negative_risk_sha256.len() != 64
        || manifest.source_snapshots.relayer_sha256.len() != 64
        || manifest
            .activation
            .requires_competing_same_nonce_conformance
            != true
        || manifest.activation.has_active_caller
        || manifest.activation.has_durable_state
    {
        return Err(RedemptionConfigError::ManifestDrift);
    }
    Ok(())
}

fn parse_address(
    value: &str,
    allow_zero: bool,
) -> Result<[u8; ADDRESS_BYTES], RedemptionConfigError> {
    let bytes = parse_fixed(value)?;
    if !allow_zero && bytes == [0; ADDRESS_BYTES] {
        return Err(RedemptionConfigError::InvalidAddress);
    }
    Ok(bytes)
}

fn parse_fixed<const N: usize>(value: &str) -> Result<[u8; N], RedemptionConfigError> {
    let encoded = value
        .strip_prefix("0x")
        .ok_or(RedemptionConfigError::InvalidAddress)?;
    let mut output = [0; N];
    hex::decode_to_slice(encoded, &mut output)
        .map_err(|_| RedemptionConfigError::InvalidAddress)?;
    Ok(output)
}

fn is_zero_address(value: &str) -> bool {
    parse_address(value, true).is_ok_and(|address| address == [0; ADDRESS_BYTES])
}

mod credential_source_private {
    pub trait Sealed {}
}

/// SSM-owned streaming acquisition boundary with no production implementation
/// in this mechanically disabled slice.
pub trait CappedSsmCredentialSource: credential_source_private::Sealed {
    fn acquire(
        &mut self,
        region: &str,
        parameter_path: &str,
        sink: &mut CredentialSink<'_>,
    ) -> Result<(), RedemptionConfigError>;
}

pub struct CredentialSink<'a> {
    storage: &'a mut [u8],
    len: usize,
}

impl CredentialSink<'_> {
    pub fn append(&mut self, chunk: &[u8]) -> Result<(), RedemptionConfigError> {
        let end = self
            .len
            .checked_add(chunk.len())
            .filter(|end| *end <= self.storage.len())
            .ok_or(RedemptionConfigError::SecretBound)?;
        self.storage[self.len..end].copy_from_slice(chunk);
        self.len = end;
        Ok(())
    }
}

pub struct ResolvedRedemptionCredentials {
    signer_private_key: Zeroizing<Box<[u8]>>,
    builder_api_key: Zeroizing<Box<[u8]>>,
    builder_api_secret: Zeroizing<Box<[u8]>>,
    builder_passphrase: Zeroizing<Box<[u8]>>,
    redaction_hmac_key: Zeroizing<Box<[u8]>>,
    key_version: u32,
}

impl ResolvedRedemptionCredentials {
    pub fn projection(&self) -> RedactedProjection {
        let mut digest = Hmac::<Sha256>::new_from_slice(self.redaction_hmac_key.as_ref())
            .expect("HMAC accepts every key length");
        let mut byte_len = 0;
        for value in [
            self.signer_private_key.as_ref(),
            self.builder_api_key.as_ref(),
            self.builder_api_secret.as_ref(),
            self.builder_passphrase.as_ref(),
            self.redaction_hmac_key.as_ref(),
        ] {
            digest.update(value);
            byte_len += value.len();
        }
        RedactedProjection {
            class: ProjectionClass::Credentials,
            item_count: 5,
            byte_len,
            keyed_digest: digest.finalize().into(),
            key_version: self.key_version,
        }
    }

    pub(super) fn signer_private_key(&self) -> &[u8] {
        self.signer_private_key.as_ref()
    }

    pub(super) fn builder_api_key(&self) -> &[u8] {
        self.builder_api_key.as_ref()
    }

    pub(super) fn builder_api_secret(&self) -> &[u8] {
        self.builder_api_secret.as_ref()
    }

    pub(super) fn builder_passphrase(&self) -> &[u8] {
        self.builder_passphrase.as_ref()
    }

    pub(super) fn key_version(&self) -> u32 {
        self.key_version
    }

    pub(super) fn redaction_hmac_key(&self) -> &[u8] {
        self.redaction_hmac_key.as_ref()
    }
}

pub fn resolve_credentials(
    profile: &ValidatedRedemptionProfile,
    region: &str,
    source: &mut impl CappedSsmCredentialSource,
) -> Result<ResolvedRedemptionCredentials, RedemptionConfigError> {
    fn one(
        region: &str,
        path: &str,
        max_bytes: usize,
        max_acquisition_bytes: usize,
        source: &mut impl CappedSsmCredentialSource,
    ) -> Result<Zeroizing<Box<[u8]>>, RedemptionConfigError> {
        let mut acquisition = Zeroizing::new(vec![0; max_acquisition_bytes].into_boxed_slice());
        let mut sink = CredentialSink {
            storage: &mut acquisition,
            len: 0,
        };
        source.acquire(region, path, &mut sink)?;
        if sink.len == 0 || sink.len > max_bytes {
            return Err(RedemptionConfigError::SecretBound);
        }
        let mut exact = Zeroizing::new(vec![0; sink.len].into_boxed_slice());
        exact.copy_from_slice(&sink.storage[..sink.len]);
        Ok(exact)
    }

    let paths = &profile.credential_paths;
    Ok(ResolvedRedemptionCredentials {
        signer_private_key: one(
            region,
            &paths[0],
            profile.max_credential_bytes,
            profile.max_credential_acquisition_bytes,
            source,
        )?,
        builder_api_key: one(
            region,
            &paths[1],
            profile.max_credential_bytes,
            profile.max_credential_acquisition_bytes,
            source,
        )?,
        builder_api_secret: one(
            region,
            &paths[2],
            profile.max_credential_bytes,
            profile.max_credential_acquisition_bytes,
            source,
        )?,
        builder_passphrase: one(
            region,
            &paths[3],
            profile.max_credential_bytes,
            profile.max_credential_acquisition_bytes,
            source,
        )?,
        redaction_hmac_key: one(
            region,
            &paths[4],
            profile.max_credential_bytes,
            profile.max_credential_acquisition_bytes,
            source,
        )?,
        key_version: profile.key_version,
    })
}

#[cfg(test)]
pub(super) struct HermeticCredentialSource {
    producer: fn(&str, &mut CredentialSink<'_>) -> Result<(), RedemptionConfigError>,
}

#[cfg(test)]
impl HermeticCredentialSource {
    pub(super) fn new(
        producer: fn(&str, &mut CredentialSink<'_>) -> Result<(), RedemptionConfigError>,
    ) -> Self {
        Self { producer }
    }
}

#[cfg(test)]
impl credential_source_private::Sealed for HermeticCredentialSource {}

#[cfg(test)]
impl CappedSsmCredentialSource for HermeticCredentialSource {
    fn acquire(
        &mut self,
        _region: &str,
        parameter_path: &str,
        sink: &mut CredentialSink<'_>,
    ) -> Result<(), RedemptionConfigError> {
        (self.producer)(parameter_path, sink)
    }
}
