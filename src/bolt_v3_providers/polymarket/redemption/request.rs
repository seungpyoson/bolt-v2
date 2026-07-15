use std::fmt;

use alloy_primitives::keccak256;
use serde::Serialize;

use super::config::ValidatedRedemptionProfile;

const ADDRESS_BYTES: usize = 20;
const WORD_BYTES: usize = 32;
const SELECTOR_BYTES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketMode {
    Standard,
    NegativeRisk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedemptionBuildInput<'a> {
    pub mode: MarketMode,
    pub owner_address: &'a str,
    pub condition_id: &'a str,
    pub safe_nonce: u64,
    pub pre_balances: [[u8; WORD_BYTES]; 2],
    pub metadata: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionIdentity {
    pub chain_id: u64,
    pub wallet_type: &'static str,
    pub safe_address: String,
    pub safe_nonce: u64,
    pub target: String,
    pub calldata_hash: [u8; WORD_BYTES],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedRequestDescriptor {
    pub identity: ActionIdentity,
    pub safe_transaction_hash: [u8; WORD_BYTES],
    pub byte_len: usize,
    pub body_sha256: [u8; WORD_BYTES],
}

#[derive(Clone, PartialEq, Eq)]
pub struct SensitiveRequestBytes {
    identity: ActionIdentity,
    safe_transaction_hash: [u8; WORD_BYTES],
    bytes: Vec<u8>,
}

impl fmt::Debug for SensitiveRequestBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SensitiveRequestBytes")
            .field("descriptor", &self.descriptor())
            .finish()
    }
}

impl SensitiveRequestBytes {
    pub fn descriptor(&self) -> RedactedRequestDescriptor {
        use sha2::{Digest, Sha256};
        let digest: [u8; WORD_BYTES] = Sha256::digest(&self.bytes).into();
        RedactedRequestDescriptor {
            identity: self.identity.clone(),
            safe_transaction_hash: self.safe_transaction_hash,
            byte_len: self.bytes.len(),
            body_sha256: digest,
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnsignedRelayerRequest {
    identity: ActionIdentity,
    safe_transaction_hash: [u8; WORD_BYTES],
    from: String,
    to: String,
    proxy_wallet: String,
    data: String,
    nonce: String,
    metadata: String,
}

impl fmt::Debug for UnsignedRelayerRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnsignedRelayerRequest")
            .field("identity", &self.identity)
            .field("safe_transaction_hash", &self.safe_transaction_hash)
            .finish_non_exhaustive()
    }
}

impl UnsignedRelayerRequest {
    pub fn identity(&self) -> &ActionIdentity {
        &self.identity
    }

    pub fn safe_transaction_hash(&self) -> [u8; WORD_BYTES] {
        self.safe_transaction_hash
    }

    pub fn finalize(
        &self,
        profile: &ValidatedRedemptionProfile,
        packed_signature: &[u8],
    ) -> Result<SensitiveRequestBytes, RedemptionRequestError> {
        if packed_signature.len() != profile.manifest.safe.signature_bytes {
            return Err(RedemptionRequestError::SignatureLength);
        }
        let zero_address = normalize_address(&profile.manifest.safe.gas_token)?;
        if zero_address != normalize_address(&profile.manifest.safe.refund_receiver)? {
            return Err(RedemptionRequestError::ManifestDrift);
        }
        let wire = WireRequest {
            transaction_type: "SAFE",
            from: &self.from,
            to: &self.to,
            proxy_wallet: &self.proxy_wallet,
            data: &self.data,
            nonce: &self.nonce,
            signature: format!("0x{}", hex::encode(packed_signature)),
            signature_params: WireSignatureParams {
                gas_price: &profile.manifest.safe.gas_price,
                operation: "0",
                safe_tx_gas: &profile.manifest.safe.safe_tx_gas,
                base_gas: &profile.manifest.safe.base_gas,
                gas_token: &zero_address,
                refund_receiver: &zero_address,
            },
            metadata: &self.metadata,
        };
        let bytes =
            serde_json::to_vec(&wire).map_err(|_| RedemptionRequestError::SerializationFailure)?;
        if bytes.len() > profile.config.relayer.max_request_bytes {
            return Err(RedemptionRequestError::RequestTooLarge);
        }
        Ok(SensitiveRequestBytes {
            identity: self.identity.clone(),
            safe_transaction_hash: self.safe_transaction_hash,
            bytes,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequestPair {
    pub original: UnsignedRelayerRequest,
    pub fence: UnsignedRelayerRequest,
    pub condition_id: [u8; WORD_BYTES],
    pub pre_balances: [[u8; WORD_BYTES]; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedemptionRequestError {
    InvalidAddress,
    InvalidBytes32,
    InvalidSelector,
    MetadataTooLarge,
    SignatureLength,
    RequestTooLarge,
    SerializationFailure,
    ManifestDrift,
    RetryMismatch,
    PreSendDrift,
    ExclusiveLeaseUnavailable,
}

impl fmt::Display for RedemptionRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for RedemptionRequestError {}

pub fn build_request_pair(
    profile: &ValidatedRedemptionProfile,
    input: RedemptionBuildInput<'_>,
) -> Result<PreparedRequestPair, RedemptionRequestError> {
    if input.metadata.len() > profile.config.relayer.max_metadata_bytes {
        return Err(RedemptionRequestError::MetadataTooLarge);
    }
    let from = normalize_address(input.owner_address)?;
    let safe_address = normalize_address(&profile.config.wallet.safe_address)?;
    let condition_id = decode_fixed::<WORD_BYTES>(input.condition_id)?;
    let target = match input.mode {
        MarketMode::Standard => &profile.config.adapter.standard_target,
        MarketMode::NegativeRisk => &profile.config.adapter.negative_risk_target,
    };
    let target = normalize_address(target)?;
    let calldata = encode_redemption_calldata(profile, condition_id)?;
    let original = build_unsigned(
        profile,
        &from,
        &safe_address,
        input.safe_nonce,
        &target,
        &calldata,
        input.metadata,
    )?;
    let fence_calldata = decode_fixed::<SELECTOR_BYTES>(&profile.manifest.safe.nonce_selector)?;
    let fence = build_unsigned(
        profile,
        &from,
        &safe_address,
        input.safe_nonce,
        &safe_address,
        &fence_calldata,
        input.metadata,
    )?;
    Ok(PreparedRequestPair {
        original,
        fence,
        condition_id,
        pre_balances: input.pre_balances,
    })
}

pub fn require_exact_retry(
    original: &SensitiveRequestBytes,
    candidate: &SensitiveRequestBytes,
) -> Result<(), RedemptionRequestError> {
    if original.identity == candidate.identity && original.bytes == candidate.bytes {
        Ok(())
    } else {
        Err(RedemptionRequestError::RetryMismatch)
    }
}

pub fn revalidate_pre_send(
    prepared: &PreparedRequestPair,
    current_balances: [[u8; WORD_BYTES]; 2],
    exclusive_condition_lease_held: bool,
) -> Result<(), RedemptionRequestError> {
    if !exclusive_condition_lease_held {
        return Err(RedemptionRequestError::ExclusiveLeaseUnavailable);
    }
    if prepared.pre_balances != current_balances {
        return Err(RedemptionRequestError::PreSendDrift);
    }
    Ok(())
}

fn encode_redemption_calldata(
    profile: &ValidatedRedemptionProfile,
    condition_id: [u8; WORD_BYTES],
) -> Result<Vec<u8>, RedemptionRequestError> {
    let selector = decode_fixed::<SELECTOR_BYTES>(&profile.manifest.adapter.external_selector)?;
    let dummy_account = decode_address(&profile.config.adapter.dummy_account)?;
    let parent = decode_fixed::<WORD_BYTES>(&profile.config.adapter.dummy_parent_collection_id)?;
    let mut calldata = Vec::with_capacity(
        SELECTOR_BYTES + WORD_BYTES * (5 + profile.config.adapter.dummy_index_sets.len()),
    );
    calldata.extend_from_slice(&selector);
    calldata.extend_from_slice(&address_word(dummy_account));
    calldata.extend_from_slice(&parent);
    calldata.extend_from_slice(&condition_id);
    calldata.extend_from_slice(&u64_word((WORD_BYTES * 4) as u64));
    calldata.extend_from_slice(&u64_word(
        profile.config.adapter.dummy_index_sets.len() as u64
    ));
    for index_set in &profile.config.adapter.dummy_index_sets {
        calldata.extend_from_slice(&u64_word(*index_set));
    }
    Ok(calldata)
}

fn build_unsigned(
    profile: &ValidatedRedemptionProfile,
    from: &str,
    safe_address: &str,
    safe_nonce: u64,
    target: &str,
    calldata: &[u8],
    metadata: &str,
) -> Result<UnsignedRelayerRequest, RedemptionRequestError> {
    let calldata_hash = keccak256(calldata).0;
    let identity = ActionIdentity {
        chain_id: profile.config.wallet.chain_id,
        wallet_type: "SAFE",
        safe_address: safe_address.to_string(),
        safe_nonce,
        target: target.to_string(),
        calldata_hash,
    };
    let safe_transaction_hash = safe_transaction_hash(
        profile.config.wallet.chain_id,
        safe_address,
        target,
        calldata,
        safe_nonce,
    )?;
    Ok(UnsignedRelayerRequest {
        identity,
        safe_transaction_hash,
        from: from.to_string(),
        to: target.to_string(),
        proxy_wallet: safe_address.to_string(),
        data: format!("0x{}", hex::encode(calldata)),
        nonce: safe_nonce.to_string(),
        metadata: metadata.to_string(),
    })
}

fn safe_transaction_hash(
    chain_id: u64,
    safe_address: &str,
    target: &str,
    calldata: &[u8],
    nonce: u64,
) -> Result<[u8; WORD_BYTES], RedemptionRequestError> {
    let domain_type_hash = keccak256(b"EIP712Domain(uint256 chainId,address verifyingContract)");
    let safe_tx_type_hash = keccak256(
        b"SafeTx(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 nonce)",
    );
    let safe = decode_address(safe_address)?;
    let to = decode_address(target)?;
    let mut domain = Vec::with_capacity(WORD_BYTES * 3);
    domain.extend_from_slice(domain_type_hash.as_slice());
    domain.extend_from_slice(&u64_word(chain_id));
    domain.extend_from_slice(&address_word(safe));
    let domain_hash = keccak256(domain);

    let mut structure = Vec::with_capacity(WORD_BYTES * 11);
    structure.extend_from_slice(safe_tx_type_hash.as_slice());
    structure.extend_from_slice(&address_word(to));
    structure.extend_from_slice(&u64_word(0));
    structure.extend_from_slice(keccak256(calldata).as_slice());
    structure.extend_from_slice(&u64_word(0));
    structure.extend_from_slice(&u64_word(0));
    structure.extend_from_slice(&u64_word(0));
    structure.extend_from_slice(&u64_word(0));
    structure.extend_from_slice(&address_word([0; ADDRESS_BYTES]));
    structure.extend_from_slice(&address_word([0; ADDRESS_BYTES]));
    structure.extend_from_slice(&u64_word(nonce));
    let structure_hash = keccak256(structure);

    let mut digest_input = Vec::with_capacity(2 + WORD_BYTES * 2);
    digest_input.extend_from_slice(&[0x19, 0x01]);
    digest_input.extend_from_slice(domain_hash.as_slice());
    digest_input.extend_from_slice(structure_hash.as_slice());
    Ok(keccak256(digest_input).0)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireRequest<'a> {
    #[serde(rename = "type")]
    transaction_type: &'static str,
    from: &'a str,
    to: &'a str,
    proxy_wallet: &'a str,
    data: &'a str,
    nonce: &'a str,
    signature: String,
    signature_params: WireSignatureParams<'a>,
    metadata: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireSignatureParams<'a> {
    gas_price: &'a str,
    operation: &'static str,
    safe_tx_gas: &'a str,
    base_gas: &'a str,
    gas_token: &'a str,
    refund_receiver: &'a str,
}

fn decode_address(value: &str) -> Result<[u8; ADDRESS_BYTES], RedemptionRequestError> {
    decode_fixed(value)
}

fn normalize_address(value: &str) -> Result<String, RedemptionRequestError> {
    Ok(format!("0x{}", hex::encode(decode_address(value)?)))
}

fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N], RedemptionRequestError> {
    let encoded = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(encoded).map_err(|_| match N {
        ADDRESS_BYTES => RedemptionRequestError::InvalidAddress,
        WORD_BYTES => RedemptionRequestError::InvalidBytes32,
        _ => RedemptionRequestError::InvalidSelector,
    })?;
    bytes.try_into().map_err(|_| match N {
        ADDRESS_BYTES => RedemptionRequestError::InvalidAddress,
        WORD_BYTES => RedemptionRequestError::InvalidBytes32,
        _ => RedemptionRequestError::InvalidSelector,
    })
}

fn address_word(address: [u8; ADDRESS_BYTES]) -> [u8; WORD_BYTES] {
    let mut word = [0; WORD_BYTES];
    word[WORD_BYTES - ADDRESS_BYTES..].copy_from_slice(&address);
    word
}

fn u64_word(value: u64) -> [u8; WORD_BYTES] {
    let mut word = [0; WORD_BYTES];
    word[WORD_BYTES - std::mem::size_of::<u64>()..].copy_from_slice(&value.to_be_bytes());
    word
}
