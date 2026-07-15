use std::io::Read;

use alloy_primitives::keccak256;
use base64::{Engine, engine::general_purpose};
use hmac::{Hmac, Mac};
use k256::ecdsa::SigningKey;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::bounded::{CappedBytes, CappedIoError, ProjectionClass, RedactedProjection};
use super::capability::{
    ExactConditionSnapshotLease, FenceMayHaveStartedPermit, FreshPreSendValidation,
    OriginalMayHaveStartedPermit, SafeNonceBodyCapacityPermit,
};
use super::config::{ResolvedRedemptionCredentials, ValidatedRedemptionProfile};
use super::nonce::SafeNonce;

const ADDRESS_BYTES: usize = 20;
const WORD_BYTES: usize = 32;
const SELECTOR_BYTES: usize = 4;
const INDEX_SET_COUNT: usize = 2;
const CALLDATA_BYTES: usize = SELECTOR_BYTES + WORD_BYTES * (5 + INDEX_SET_COUNT);
const SIGNATURE_BYTES: usize = 65;
const MAX_NONCE_DECIMAL_BYTES: usize = 78;
const MAX_U64_DECIMAL_BYTES: usize = 20;
const BASE64_HMAC_BYTES: usize = 44;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketMode {
    Standard,
    NegativeRisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Original,
    Fence,
}

pub struct RedemptionBuildInput<'a> {
    mode: MarketMode,
    owner_address: &'a str,
    metadata: &'a str,
}

impl<'a> RedemptionBuildInput<'a> {
    pub fn new(mode: MarketMode, owner_address: &'a str, metadata: &'a str) -> Self {
        Self {
            mode,
            owner_address,
            metadata,
        }
    }
}

pub(super) struct ActionIdentity {
    chain_id: u64,
    safe_address: [u8; ADDRESS_BYTES],
    nonce: SafeNonce,
    target: [u8; ADDRESS_BYTES],
    calldata_hash: [u8; WORD_BYTES],
    safe_transaction_hash: [u8; WORD_BYTES],
}

impl ActionIdentity {
    pub(super) fn safe_address(&self) -> [u8; ADDRESS_BYTES] {
        self.safe_address
    }

    pub(super) fn nonce(&self) -> SafeNonce {
        self.nonce
    }

    pub(super) fn target(&self) -> [u8; ADDRESS_BYTES] {
        self.target
    }

    pub(super) fn safe_transaction_hash(&self) -> [u8; WORD_BYTES] {
        self.safe_transaction_hash
    }
}

impl Drop for ActionIdentity {
    fn drop(&mut self) {
        self.chain_id.zeroize();
        self.safe_address.zeroize();
        self.nonce.zeroize();
        self.target.zeroize();
        self.calldata_hash.zeroize();
        self.safe_transaction_hash.zeroize();
    }
}

pub(super) struct SignedRequest {
    identity: ActionIdentity,
    owner: [u8; ADDRESS_BYTES],
    calldata: [u8; CALLDATA_BYTES],
    calldata_len: usize,
    metadata: Zeroizing<Box<[u8]>>,
    body: CappedBytes,
    headers: CappedBytes,
}

impl SignedRequest {
    pub(super) fn identity(&self) -> &ActionIdentity {
        &self.identity
    }

    pub(super) fn owner(&self) -> [u8; ADDRESS_BYTES] {
        self.owner
    }

    pub(super) fn calldata(&self) -> &[u8] {
        &self.calldata[..self.calldata_len]
    }

    pub(super) fn metadata(&self) -> &[u8] {
        &self.metadata
    }

    fn projection(&self, credentials: &ResolvedRedemptionCredentials) -> RedactedProjection {
        self.body.projection(
            ProjectionClass::RequestBody,
            1,
            credentials.redaction_hmac_key(),
            credentials.key_version(),
        )
    }
}

impl Drop for SignedRequest {
    fn drop(&mut self) {
        self.owner.zeroize();
        self.calldata.zeroize();
        self.calldata_len.zeroize();
    }
}

pub struct PreparedRequestPair {
    original: SignedRequest,
    fence: SignedRequest,
    condition_id: [u8; WORD_BYTES],
    pre_balances: [[u8; WORD_BYTES]; 2],
    action_digest: [u8; WORD_BYTES],
    snapshot_generation: u64,
    lane_generation: u64,
    _snapshot_lease: ExactConditionSnapshotLease,
    _nonce_capacity: SafeNonceBodyCapacityPermit,
}

pub struct AuthorizedRequest<'a> {
    request: &'a SignedRequest,
}

pub struct OriginalMayHaveStartedRequest {
    prepared: PreparedRequestPair,
    _fresh: FreshPreSendValidation,
    _durable: OriginalMayHaveStartedPermit,
}

pub struct FenceMayHaveStartedRequest {
    original: OriginalMayHaveStartedRequest,
    _fresh: FreshPreSendValidation,
    _durable: FenceMayHaveStartedPermit,
}

impl PreparedRequestPair {
    pub fn request_projection(
        &self,
        kind: RequestKind,
        credentials: &ResolvedRedemptionCredentials,
    ) -> RedactedProjection {
        self.request(kind).projection(credentials)
    }

    pub fn same_nonce(&self) -> bool {
        self.original.identity.nonce == self.fence.identity.nonce
    }

    pub(super) fn request(&self, kind: RequestKind) -> &SignedRequest {
        match kind {
            RequestKind::Original => &self.original,
            RequestKind::Fence => &self.fence,
        }
    }

    pub(super) fn condition_id(&self) -> [u8; WORD_BYTES] {
        self.condition_id
    }

    pub(super) fn pre_balances(&self) -> [[u8; WORD_BYTES]; 2] {
        self.pre_balances
    }

    pub(super) fn action_digest(&self) -> [u8; WORD_BYTES] {
        self.action_digest
    }

    pub(super) fn safe_nonce(&self) -> SafeNonce {
        self.original.identity.nonce()
    }

    pub(super) fn body_hashes(&self) -> [[u8; WORD_BYTES]; 2] {
        [self.original.body.hash(), self.fence.body.hash()]
    }

    pub fn authorize_original(
        self,
        fresh: FreshPreSendValidation,
        durable: OriginalMayHaveStartedPermit,
    ) -> Result<OriginalMayHaveStartedRequest, RedemptionRequestError> {
        if !fresh.matches(
            self.action_digest,
            self.snapshot_generation,
            self.lane_generation,
        ) || !durable.matches(self.action_digest, self.original.body.hash())
        {
            return Err(RedemptionRequestError::CapabilityMismatch);
        }
        Ok(OriginalMayHaveStartedRequest {
            prepared: self,
            _fresh: fresh,
            _durable: durable,
        })
    }

    #[cfg(test)]
    pub(super) fn hermetic_bindings(
        &self,
    ) -> (
        [u8; WORD_BYTES],
        [u8; WORD_BYTES],
        [u8; WORD_BYTES],
        u64,
        u64,
    ) {
        (
            self.action_digest,
            self.original.body.hash(),
            self.fence.body.hash(),
            self.snapshot_generation,
            self.lane_generation,
        )
    }

    #[cfg(test)]
    pub(super) fn hermetic_body(&self, kind: RequestKind) -> &[u8] {
        self.request(kind).body.as_slice()
    }

    #[cfg(test)]
    pub(super) fn hermetic_headers(&self, kind: RequestKind) -> &[u8] {
        self.request(kind).headers.as_slice()
    }
}

impl OriginalMayHaveStartedRequest {
    pub fn original(&self) -> AuthorizedRequest<'_> {
        AuthorizedRequest {
            request: &self.prepared.original,
        }
    }

    pub fn authorize_fence(
        self,
        fresh: FreshPreSendValidation,
        durable: FenceMayHaveStartedPermit,
    ) -> Result<FenceMayHaveStartedRequest, RedemptionRequestError> {
        if !fresh.matches(
            self.prepared.action_digest,
            self.prepared.snapshot_generation,
            self.prepared.lane_generation,
        ) || !durable.matches(
            self.prepared.action_digest,
            self.prepared.original.body.hash(),
            self.prepared.fence.body.hash(),
        ) {
            return Err(RedemptionRequestError::CapabilityMismatch);
        }
        Ok(FenceMayHaveStartedRequest {
            original: self,
            _fresh: fresh,
            _durable: durable,
        })
    }

    pub(super) fn prepared(&self) -> &PreparedRequestPair {
        &self.prepared
    }
}

impl FenceMayHaveStartedRequest {
    pub fn original(&self) -> AuthorizedRequest<'_> {
        self.original.original()
    }

    pub fn fence(&self) -> AuthorizedRequest<'_> {
        AuthorizedRequest {
            request: &self.original.prepared.fence,
        }
    }

    pub(super) fn prepared(&self) -> &PreparedRequestPair {
        &self.original.prepared
    }
}

impl AuthorizedRequest<'_> {
    pub fn projection(&self, credentials: &ResolvedRedemptionCredentials) -> RedactedProjection {
        self.request.projection(credentials)
    }
}

impl Drop for PreparedRequestPair {
    fn drop(&mut self) {
        self.condition_id.zeroize();
        self.pre_balances.zeroize();
        self.action_digest.zeroize();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedemptionRequestError {
    InvalidAddress,
    InvalidBytes32,
    MetadataTooLarge,
    NonceExhausted,
    Signing,
    SignerMismatch,
    Authorization,
    RequestTooLarge,
    RetryMismatch,
    CapabilityMismatch,
    CapacityPermitMismatch,
    Read,
}

pub fn build_request_pair(
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    snapshot: ExactConditionSnapshotLease,
    nonce_capacity: SafeNonceBodyCapacityPermit,
    input: RedemptionBuildInput<'_>,
    builder_timestamp: u64,
) -> Result<PreparedRequestPair, RedemptionRequestError> {
    if input.metadata.len() > profile.max_metadata_bytes() {
        return Err(RedemptionRequestError::MetadataTooLarge);
    }
    let (condition_id, pre_balances, snapshot_generation) = snapshot.parts();
    let (safe_address, safe_nonce, original_capacity, fence_capacity, lane_generation) =
        nonce_capacity.parts();
    if safe_address != profile.safe_address()
        || original_capacity != profile.max_request_bytes()
        || fence_capacity != profile.max_request_bytes()
    {
        return Err(RedemptionRequestError::CapacityPermitMismatch);
    }
    if safe_nonce.is_max() {
        return Err(RedemptionRequestError::NonceExhausted);
    }
    let owner = decode_address(input.owner_address)?;
    let target = profile.target(input.mode == MarketMode::NegativeRisk);
    let original_calldata = redemption_calldata(profile, condition_id);
    let mut fence_calldata = [0u8; CALLDATA_BYTES];
    fence_calldata[..SELECTOR_BYTES].copy_from_slice(&profile.nonce_selector());
    let original = signed_request(
        profile,
        credentials,
        owner,
        target,
        &original_calldata,
        safe_nonce,
        input.metadata.as_bytes(),
        builder_timestamp,
    )?;
    let fence = signed_request(
        profile,
        credentials,
        owner,
        profile.safe_address(),
        &fence_calldata[..SELECTOR_BYTES],
        safe_nonce,
        input.metadata.as_bytes(),
        builder_timestamp,
    )?;
    let action_digest = action_digest(profile, &original, &fence, condition_id, pre_balances);
    Ok(PreparedRequestPair {
        original,
        fence,
        condition_id,
        pre_balances,
        action_digest,
        snapshot_generation,
        lane_generation,
        _snapshot_lease: snapshot,
        _nonce_capacity: nonce_capacity,
    })
}

pub fn require_exact_retry(
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    request: &AuthorizedRequest<'_>,
    body_candidate: impl Read,
    header_candidate: impl Read,
) -> Result<(), RedemptionRequestError> {
    let body_candidate = CappedBytes::read_with_probe(
        body_candidate,
        profile.max_request_bytes(),
        profile.overflow_probe_bytes(),
        credentials.redaction_hmac_key(),
        credentials.key_version(),
        ProjectionClass::RequestBody,
    )
    .map_err(map_capped_error)?;
    let header_candidate = CappedBytes::read_with_probe(
        header_candidate,
        profile.max_header_bytes(),
        profile.overflow_probe_bytes(),
        credentials.redaction_hmac_key(),
        credentials.key_version(),
        ProjectionClass::AuthorizationHeaders,
    )
    .map_err(map_capped_error)?;
    if exact_bytes_match(request.request.body.as_slice(), body_candidate.as_slice())
        && exact_bytes_match(
            request.request.headers.as_slice(),
            header_candidate.as_slice(),
        )
    {
        Ok(())
    } else {
        Err(RedemptionRequestError::RetryMismatch)
    }
}

fn exact_bytes_match(expected: &[u8], actual: &[u8]) -> bool {
    let mut difference = expected.len() ^ actual.len();
    for index in 0..expected.len().min(actual.len()) {
        difference |= usize::from(expected[index] ^ actual[index]);
    }
    difference == 0
}

fn signed_request(
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    owner: [u8; ADDRESS_BYTES],
    target: [u8; ADDRESS_BYTES],
    calldata: &[u8],
    nonce: SafeNonce,
    metadata: &[u8],
    builder_timestamp: u64,
) -> Result<SignedRequest, RedemptionRequestError> {
    let safe_transaction_hash = safe_transaction_hash(
        profile.chain_id(),
        profile.safe_address(),
        target,
        calldata,
        nonce,
    );
    let signature = sign_hash(credentials, owner, safe_transaction_hash)?;
    let identity = ActionIdentity {
        chain_id: profile.chain_id(),
        safe_address: profile.safe_address(),
        nonce,
        target,
        calldata_hash: keccak256(calldata).0,
        safe_transaction_hash,
    };
    let body = request_body(profile, owner, &identity, calldata, metadata, &signature)?;
    let headers = authorization_headers(profile, credentials, builder_timestamp, &body)?;
    let mut stored_calldata = [0; CALLDATA_BYTES];
    stored_calldata[..calldata.len()].copy_from_slice(calldata);
    Ok(SignedRequest {
        identity,
        owner,
        calldata: stored_calldata,
        calldata_len: calldata.len(),
        metadata: zeroizing_box_from_slice(metadata),
        body,
        headers,
    })
}

fn sign_hash(
    credentials: &ResolvedRedemptionCredentials,
    expected_owner: [u8; ADDRESS_BYTES],
    hash: [u8; WORD_BYTES],
) -> Result<[u8; SIGNATURE_BYTES], RedemptionRequestError> {
    let encoded = credentials
        .signer_private_key()
        .strip_prefix(b"0x")
        .unwrap_or(credentials.signer_private_key());
    let mut secret = Zeroizing::new([0u8; WORD_BYTES]);
    hex::decode_to_slice(encoded, &mut *secret).map_err(|_| RedemptionRequestError::Signing)?;
    let signing_key =
        SigningKey::from_bytes((&*secret).into()).map_err(|_| RedemptionRequestError::Signing)?;
    let public = signing_key.verifying_key().to_encoded_point(false);
    let derived = keccak256(&public.as_bytes()[1..]);
    if derived[WORD_BYTES - ADDRESS_BYTES..] != expected_owner {
        return Err(RedemptionRequestError::SignerMismatch);
    }
    let (signature, recovery_id) = signing_key
        .sign_prehash_recoverable(&hash)
        .map_err(|_| RedemptionRequestError::Signing)?;
    let mut packed = [0u8; SIGNATURE_BYTES];
    packed[..SIGNATURE_BYTES - 1].copy_from_slice(signature.to_bytes().as_slice());
    packed[SIGNATURE_BYTES - 1] = recovery_id.to_byte() + 27;
    Ok(packed)
}

fn request_body(
    profile: &ValidatedRedemptionProfile,
    owner: [u8; ADDRESS_BYTES],
    identity: &ActionIdentity,
    calldata: &[u8],
    metadata: &[u8],
    signature: &[u8; SIGNATURE_BYTES],
) -> Result<CappedBytes, RedemptionRequestError> {
    let mut nonce_decimal = [0; MAX_NONCE_DECIMAL_BYTES];
    let nonce_len = identity.nonce.write_decimal(&mut nonce_decimal);
    let mut body = CappedBytes::with_capacity(profile.max_request_bytes());
    body.extend(br#"{"type":"SAFE","from":"0x"#)
        .map_err(map_capped_error)?;
    body.append_hex(&owner).map_err(map_capped_error)?;
    body.extend(br#"","to":"0x"#).map_err(map_capped_error)?;
    body.append_hex(&identity.target)
        .map_err(map_capped_error)?;
    body.extend(br#"","proxyWallet":"0x"#)
        .map_err(map_capped_error)?;
    body.append_hex(&identity.safe_address)
        .map_err(map_capped_error)?;
    body.extend(br#"","data":"0x"#).map_err(map_capped_error)?;
    body.append_hex(calldata).map_err(map_capped_error)?;
    body.extend(br#"","nonce":""#).map_err(map_capped_error)?;
    body.extend(&nonce_decimal[..nonce_len])
        .map_err(map_capped_error)?;
    body.extend(br#"","signature":"0x"#)
        .map_err(map_capped_error)?;
    body.append_hex(signature).map_err(map_capped_error)?;
    body.extend(br#"","signatureParams":{"gasPrice":"0","operation":"0","safeTxGas":"0","baseGas":"0","gasToken":"0x0000000000000000000000000000000000000000","refundReceiver":"0x0000000000000000000000000000000000000000"},"metadata":"#)
        .map_err(map_capped_error)?;
    body.append_json_string(metadata)
        .map_err(map_capped_error)?;
    body.push(b'}').map_err(map_capped_error)?;
    Ok(body)
}

fn authorization_headers(
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    timestamp: u64,
    body: &CappedBytes,
) -> Result<CappedBytes, RedemptionRequestError> {
    let mut timestamp_bytes = [0; MAX_U64_DECIMAL_BYTES];
    let timestamp_len = write_u64_decimal(timestamp, &mut timestamp_bytes);
    let secret_len = credentials.builder_api_secret().len();
    let decoded_capacity = secret_len
        .checked_div(4)
        .and_then(|value| value.checked_add(1))
        .and_then(|value| value.checked_mul(3))
        .ok_or(RedemptionRequestError::Authorization)?;
    let mut decoded_secret = Zeroizing::new(vec![0; decoded_capacity].into_boxed_slice());
    let decoded_len = general_purpose::STANDARD
        .decode_slice(credentials.builder_api_secret(), &mut *decoded_secret)
        .map_err(|_| RedemptionRequestError::Authorization)?;
    let mut hmac = Hmac::<Sha256>::new_from_slice(&decoded_secret[..decoded_len])
        .map_err(|_| RedemptionRequestError::Authorization)?;
    hmac.update(&timestamp_bytes[..timestamp_len]);
    hmac.update(b"POST");
    hmac.update(profile.submit_path().as_bytes());
    hmac.update(body.as_slice());
    let signature = hmac.finalize().into_bytes();
    let mut encoded_signature = [0; BASE64_HMAC_BYTES];
    let signature_len = general_purpose::URL_SAFE
        .encode_slice(signature, &mut encoded_signature)
        .map_err(|_| RedemptionRequestError::Authorization)?;
    let mut headers = CappedBytes::with_capacity(profile.max_header_bytes());
    headers
        .extend(br#"{"POLY_BUILDER_API_KEY":"#)
        .map_err(map_capped_error)?;
    headers
        .append_json_string(credentials.builder_api_key())
        .map_err(map_capped_error)?;
    headers
        .extend(br#","POLY_BUILDER_TIMESTAMP":""#)
        .map_err(map_capped_error)?;
    headers
        .extend(&timestamp_bytes[..timestamp_len])
        .map_err(map_capped_error)?;
    headers
        .extend(br#"","POLY_BUILDER_PASSPHRASE":"#)
        .map_err(map_capped_error)?;
    headers
        .append_json_string(credentials.builder_passphrase())
        .map_err(map_capped_error)?;
    headers
        .extend(br#","POLY_BUILDER_SIGNATURE":""#)
        .map_err(map_capped_error)?;
    headers
        .extend(&encoded_signature[..signature_len])
        .map_err(map_capped_error)?;
    headers.extend(br#""}"#).map_err(map_capped_error)?;
    Ok(headers)
}

fn redemption_calldata(
    profile: &ValidatedRedemptionProfile,
    condition_id: [u8; WORD_BYTES],
) -> [u8; CALLDATA_BYTES] {
    let (dummy_account, parent, index_sets) = profile.adapter_arguments();
    let mut output = [0; CALLDATA_BYTES];
    let mut offset = 0;
    append_fixed(&mut output, &mut offset, &profile.redemption_selector());
    append_fixed(&mut output, &mut offset, &address_word(dummy_account));
    append_fixed(&mut output, &mut offset, &parent);
    append_fixed(&mut output, &mut offset, &condition_id);
    append_fixed(&mut output, &mut offset, &u64_word(WORD_BYTES as u64 * 4));
    append_fixed(&mut output, &mut offset, &u64_word(INDEX_SET_COUNT as u64));
    for index_set in index_sets {
        append_fixed(&mut output, &mut offset, &u64_word(index_set));
    }
    output
}

fn safe_transaction_hash(
    chain_id: u64,
    safe_address: [u8; ADDRESS_BYTES],
    target: [u8; ADDRESS_BYTES],
    calldata: &[u8],
    nonce: SafeNonce,
) -> [u8; WORD_BYTES] {
    let domain_type_hash = keccak256(b"EIP712Domain(uint256 chainId,address verifyingContract)");
    let safe_tx_type_hash = keccak256(
        b"SafeTx(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 nonce)",
    );
    let mut domain = [0; WORD_BYTES * 3];
    domain[..WORD_BYTES].copy_from_slice(domain_type_hash.as_slice());
    domain[WORD_BYTES..WORD_BYTES * 2].copy_from_slice(&u64_word(chain_id));
    domain[WORD_BYTES * 2..].copy_from_slice(&address_word(safe_address));
    let domain_hash = keccak256(domain);

    let words = [
        safe_tx_type_hash.0,
        address_word(target),
        u64_word(0),
        keccak256(calldata).0,
        u64_word(0),
        u64_word(0),
        u64_word(0),
        u64_word(0),
        address_word([0; ADDRESS_BYTES]),
        address_word([0; ADDRESS_BYTES]),
        *nonce.as_word(),
    ];
    let mut structure = [0; WORD_BYTES * 11];
    for (index, word) in words.into_iter().enumerate() {
        structure[index * WORD_BYTES..(index + 1) * WORD_BYTES].copy_from_slice(&word);
    }
    let structure_hash = keccak256(structure);
    let mut digest = [0; 2 + WORD_BYTES * 2];
    digest[..2].copy_from_slice(&[0x19, 0x01]);
    digest[2..2 + WORD_BYTES].copy_from_slice(domain_hash.as_slice());
    digest[2 + WORD_BYTES..].copy_from_slice(structure_hash.as_slice());
    keccak256(digest).0
}

fn action_digest(
    profile: &ValidatedRedemptionProfile,
    original: &SignedRequest,
    fence: &SignedRequest,
    condition_id: [u8; WORD_BYTES],
    pre_balances: [[u8; WORD_BYTES]; 2],
) -> [u8; WORD_BYTES] {
    let mut digest = Sha256::new();
    digest.update(profile.chain_id().to_be_bytes());
    digest.update(profile.safe_address());
    digest.update(condition_id);
    digest.update(pre_balances[0]);
    digest.update(pre_balances[1]);
    digest.update(original.body.as_slice());
    digest.update(original.headers.as_slice());
    digest.update(fence.body.as_slice());
    digest.update(fence.headers.as_slice());
    digest.finalize().into()
}

fn decode_address(value: &str) -> Result<[u8; ADDRESS_BYTES], RedemptionRequestError> {
    decode_fixed(value).map_err(|_| RedemptionRequestError::InvalidAddress)
}

fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N], RedemptionRequestError> {
    let encoded = value
        .strip_prefix("0x")
        .ok_or(RedemptionRequestError::InvalidBytes32)?;
    let mut output = [0; N];
    hex::decode_to_slice(encoded, &mut output)
        .map_err(|_| RedemptionRequestError::InvalidBytes32)?;
    Ok(output)
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

fn append_fixed<const N: usize>(output: &mut [u8], offset: &mut usize, value: &[u8; N]) {
    output[*offset..*offset + N].copy_from_slice(value);
    *offset += N;
}

fn write_u64_decimal(mut value: u64, output: &mut [u8; MAX_U64_DECIMAL_BYTES]) -> usize {
    if value == 0 {
        output[0] = b'0';
        return 1;
    }
    let mut reverse = [0; MAX_U64_DECIMAL_BYTES];
    let mut len = 0;
    while value != 0 {
        reverse[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    for index in 0..len {
        output[index] = reverse[len - index - 1];
    }
    len
}

fn map_capped_error(error: CappedIoError) -> RedemptionRequestError {
    match error {
        CappedIoError::Read => RedemptionRequestError::Read,
        CappedIoError::Capacity | CappedIoError::InvalidLimit | CappedIoError::Oversize(_) => {
            RedemptionRequestError::RequestTooLarge
        }
    }
}

fn zeroizing_box_from_slice(value: &[u8]) -> Zeroizing<Box<[u8]>> {
    let mut output = Zeroizing::new(vec![0; value.len()].into_boxed_slice());
    output.copy_from_slice(value);
    output
}
