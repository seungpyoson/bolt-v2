use std::fmt;
use std::io::Read;

use alloy_primitives::keccak256;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use zeroize::{Zeroize, Zeroizing};

use super::bounded::{
    CappedBytes, CappedIoError, ProjectionClass, RedactedProjection, keyed_digest,
};
use super::config::{ResolvedRedemptionCredentials, ValidatedRedemptionProfile};
use super::nonce::SafeNonce;
use super::query::{
    NonceRelation, PostStateRelation, RedemptionResolution, SourceBoundVerifiedOutcome,
    classify_nonce_successor,
};
use super::request::{
    FenceMayHaveStartedRequest, OriginalMayHaveStartedRequest, PreparedRequestPair, RequestKind,
};

const ADDRESS_BYTES: usize = 20;
const WORD_BYTES: usize = 32;

mod response_authority_private {
    pub trait Sealed {}
}

/// Sealed proof that a quorum-durable exact request may already have started.
pub trait ResponseReadAuthority: response_authority_private::Sealed {
    #[doc(hidden)]
    fn prepared_request_pair(&self) -> &PreparedRequestPair;
}

impl response_authority_private::Sealed for OriginalMayHaveStartedRequest {}
impl response_authority_private::Sealed for FenceMayHaveStartedRequest {}

impl ResponseReadAuthority for OriginalMayHaveStartedRequest {
    fn prepared_request_pair(&self) -> &PreparedRequestPair {
        self.prepared()
    }
}

impl ResponseReadAuthority for FenceMayHaveStartedRequest {
    fn prepared_request_pair(&self) -> &PreparedRequestPair {
        self.prepared()
    }
}

pub struct BoundedWireResponse {
    bytes: CappedBytes,
    class: ProjectionClass,
}

impl BoundedWireResponse {
    pub fn read_relayer(
        _authority: &impl ResponseReadAuthority,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        reader: impl Read,
    ) -> Result<Self, WireParseError> {
        Self::read(
            reader,
            profile.max_relayer_response_bytes(),
            profile,
            credentials,
            ProjectionClass::RelayerResponse,
        )
    }

    pub fn read_chain(
        _authority: &impl ResponseReadAuthority,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        reader: impl Read,
    ) -> Result<Self, WireParseError> {
        Self::read(
            reader,
            profile.max_rpc_response_bytes(),
            profile,
            credentials,
            ProjectionClass::ChainResponse,
        )
    }

    fn read(
        reader: impl Read,
        limit: usize,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        class: ProjectionClass,
    ) -> Result<Self, WireParseError> {
        let bytes = CappedBytes::read_with_probe(
            reader,
            limit,
            profile.overflow_probe_bytes(),
            credentials.redaction_hmac_key(),
            credentials.key_version(),
            class,
        )
        .map_err(|error| WireParseError::from_capped(error, class))?;
        Ok(Self { bytes, class })
    }

    pub fn projection(&self, credentials: &ResolvedRedemptionCredentials) -> RedactedProjection {
        self.bytes.projection(
            self.class,
            1,
            credentials.redaction_hmac_key(),
            credentials.key_version(),
        )
    }

    pub fn parse_submit(
        &self,
        _authority: &impl ResponseReadAuthority,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
    ) -> Result<RelayerObservation, WireParseError> {
        if self.class != ProjectionClass::RelayerResponse {
            return Err(self.failure(WireFailureClass::IdentityMismatch, credentials));
        }
        let parsed: SubmitWire<'_> = serde_json::from_slice(self.bytes.as_slice())
            .map_err(|_| self.failure(WireFailureClass::Malformed, credentials))?;
        validate_id(parsed.transaction_id, profile)
            .map_err(|class| self.failure(class, credentials))?;
        let state = parse_state(parsed.state)
            .ok_or_else(|| self.failure(WireFailureClass::UnknownState, credentials))?;
        let transaction_hash = parse_optional_hash(parsed.transaction_hash)
            .ok_or_else(|| self.failure(WireFailureClass::Malformed, credentials))?;
        Ok(RelayerObservation {
            transaction_id: zeroizing_box(parsed.transaction_id.as_bytes()),
            transaction_id_digest: keyed_digest(
                credentials.redaction_hmac_key(),
                parsed.transaction_id.as_bytes(),
            ),
            state,
            transaction_hash,
            key_version: credentials.key_version(),
        })
    }

    pub fn parse_exact_transaction(
        &self,
        authority: &impl ResponseReadAuthority,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        expected: &RelayerObservation,
        kind: RequestKind,
    ) -> Result<RelayerObservation, WireParseError> {
        if self.class != ProjectionClass::RelayerResponse {
            return Err(self.failure(WireFailureClass::IdentityMismatch, credentials));
        }
        let exact: ExactOne<'_> = serde_json::from_slice(self.bytes.as_slice())
            .map_err(|_| self.failure(WireFailureClass::Malformed, credentials))?;
        let transaction = exact.0;
        validate_id(transaction.transaction_id, profile)
            .map_err(|class| self.failure(class, credentials))?;
        if transaction.transaction_id.as_bytes() != expected.transaction_id.as_ref() {
            return Err(self.failure(WireFailureClass::IdentityMismatch, credentials));
        }
        let request = authority.prepared_request_pair().request(kind);
        if decode_address(transaction.from) != Some(request.owner())
            || decode_address(transaction.to) != Some(request.identity().target())
            || decode_address(transaction.proxy_address) != Some(request.identity().safe_address())
            || !hex_matches(transaction.data, request.calldata())
            || SafeNonce::from_decimal(transaction.nonce).ok() != Some(request.identity().nonce())
            || transaction.value != "0"
            || transaction.transaction_type != "SAFE"
            || transaction.metadata.as_bytes() != request.metadata()
        {
            return Err(self.failure(WireFailureClass::IdentityMismatch, credentials));
        }
        if transaction.created_at.len() > profile.max_timestamp_bytes()
            || transaction.updated_at.len() > profile.max_timestamp_bytes()
            || transaction.metadata.len() > profile.max_metadata_bytes()
        {
            return Err(self.failure(WireFailureClass::FieldTooLarge, credentials));
        }
        let state = parse_state(transaction.state)
            .ok_or_else(|| self.failure(WireFailureClass::UnknownState, credentials))?;
        let transaction_hash = parse_optional_hash(transaction.transaction_hash)
            .ok_or_else(|| self.failure(WireFailureClass::Malformed, credentials))?;
        if transaction_hash.is_some_and(|hash| hash != request.identity().safe_transaction_hash())
            || expected
                .transaction_hash
                .is_some_and(|expected_hash| Some(expected_hash) != transaction_hash)
        {
            return Err(self.failure(WireFailureClass::IdentityMismatch, credentials));
        }
        Ok(RelayerObservation {
            transaction_id: zeroizing_box(transaction.transaction_id.as_bytes()),
            transaction_id_digest: expected.transaction_id_digest,
            state,
            transaction_hash,
            key_version: expected.key_version,
        })
    }

    fn failure(
        &self,
        class: WireFailureClass,
        credentials: &ResolvedRedemptionCredentials,
    ) -> WireParseError {
        WireParseError {
            diagnostic: WireDiagnostic {
                class,
                http_status: None,
                projection: self.projection(credentials),
            },
        }
    }
}

pub struct ExactQueryResponses {
    nonce: BoundedWireResponse,
    original_execution: BoundedWireResponse,
    fence_execution: BoundedWireResponse,
    post_state: BoundedWireResponse,
    safe_boundary: BoundedWireResponse,
    finalized_head: BoundedWireResponse,
}

impl ExactQueryResponses {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        _authority: &impl ResponseReadAuthority,
        credentials: &ResolvedRedemptionCredentials,
        nonce: BoundedWireResponse,
        original_execution: BoundedWireResponse,
        fence_execution: BoundedWireResponse,
        post_state: BoundedWireResponse,
        safe_boundary: BoundedWireResponse,
        finalized_head: BoundedWireResponse,
    ) -> Result<Self, WireParseError> {
        for response in [
            &nonce,
            &original_execution,
            &fence_execution,
            &post_state,
            &safe_boundary,
            &finalized_head,
        ] {
            if response.class != ProjectionClass::ChainResponse {
                return Err(response.failure(WireFailureClass::IdentityMismatch, credentials));
            }
        }
        Ok(Self {
            nonce,
            original_execution,
            fence_execution,
            post_state,
            safe_boundary,
            finalized_head,
        })
    }

    pub fn verify_after_original(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        attempt: &OriginalMayHaveStartedRequest,
    ) -> Result<SourceBoundVerifiedOutcome, WireParseError> {
        self.verify(profile, credentials, attempt.prepared(), false)
    }

    pub fn verify_after_fence(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        attempt: &FenceMayHaveStartedRequest,
    ) -> Result<SourceBoundVerifiedOutcome, WireParseError> {
        self.verify(profile, credentials, attempt.prepared(), true)
    }

    fn verify(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        prepared: &PreparedRequestPair,
        fence_durably_authorized: bool,
    ) -> Result<SourceBoundVerifiedOutcome, WireParseError> {
        let nonce: NonceCallWire<'_> = self.parse(&self.nonce, credentials)?;
        let original: ExecutionQueryWire<'_> = self.parse(&self.original_execution, credentials)?;
        let fence: ExecutionQueryWire<'_> = self.parse(&self.fence_execution, credentials)?;
        let post: PostStateWire<'_> = self.parse(&self.post_state, credentials)?;
        let boundary: SafeBoundaryWire<'_> = self.parse(&self.safe_boundary, credentials)?;
        let finalized: FinalizedHeadWire<'_> = self.parse(&self.finalized_head, credentials)?;

        let finalized_number = parse_quantity_word(finalized.block_number);
        let finalized_hash = parse_word(finalized.block_hash);
        let nonce_number = parse_quantity_word(nonce.block_number);
        let nonce_hash = parse_word(nonce.block_hash);
        let post_number = parse_quantity_word(post.block_number);
        let post_hash = parse_word(post.block_hash);
        let boundary_number = parse_quantity_word(boundary.block_number);
        let boundary_hash = parse_word(boundary.block_hash);
        let on_chain_nonce = parse_word(nonce.result).map(SafeNonce::from_be_bytes);
        let post_balances = [parse_word(post.results[0]), parse_word(post.results[1])];
        let (factory, implementation, fallback_handler, guard) = profile.safe_boundary();
        let common_identity_matches = finalized.query_id == "finalized_head"
            && finalized.chain_id == profile.chain_id()
            && finalized_number.is_some_and(|value| value != [0; WORD_BYTES])
            && finalized_hash.is_some_and(|value| value != [0; WORD_BYTES])
            && nonce.query_id == "safe_nonce"
            && decode_address(nonce.safe_address) == Some(profile.safe_address())
            && hex_matches(nonce.calldata, &profile.nonce_selector())
            && nonce_number == finalized_number
            && nonce_hash == finalized_hash
            && post.query_id == "raw_post_state"
            && decode_address(post.target)
                == Some(prepared.request(RequestKind::Original).identity().target())
            && parse_word(post.condition_id) == Some(prepared.condition_id())
            && post_number == finalized_number
            && post_hash == finalized_hash
            && boundary.query_id == "safe_boundary"
            && decode_address(boundary.safe) == Some(profile.safe_address())
            && decode_address(boundary.factory) == Some(factory)
            && decode_address(boundary.implementation) == Some(implementation)
            && decode_address(boundary.fallback_handler) == Some(fallback_handler)
            && decode_address(boundary.guard) == Some(guard)
            && boundary.modules.is_empty()
            && boundary_number == finalized_number
            && boundary_hash == finalized_hash;

        let original_receipt = validate_execution_query(
            &original,
            "original_finalized_receipt_logs",
            profile.safe_address(),
            prepared
                .request(RequestKind::Original)
                .identity()
                .safe_transaction_hash(),
            finalized_number,
            finalized_hash,
            profile.finality_confirmations(),
            profile.max_receipt_logs(),
        );
        let fence_receipt = validate_execution_query(
            &fence,
            "fence_finalized_receipt_logs",
            profile.safe_address(),
            prepared
                .request(RequestKind::Fence)
                .identity()
                .safe_transaction_hash(),
            finalized_number,
            finalized_hash,
            profile.finality_confirmations(),
            profile.max_receipt_logs(),
        );
        let execution_identities_match = original_receipt.is_some() && fence_receipt.is_some();
        let original_present = original_receipt.flatten();
        let fence_present = fence_receipt.flatten();
        let post_state = match post_balances {
            [Some(first), Some(second)] if [first, second] == [[0; WORD_BYTES]; 2] => {
                Some(PostStateRelation::Redeemed)
            }
            [Some(first), Some(second)] if [first, second] == prepared.pre_balances() => {
                Some(PostStateRelation::Unchanged)
            }
            [Some(_), Some(_)] => Some(PostStateRelation::Drifted),
            _ => None,
        };
        let nonce_relation = on_chain_nonce.map(|observed| {
            classify_nonce_successor(
                prepared.request(RequestKind::Original).identity().nonce(),
                observed,
            )
        });
        let resolution = if !common_identity_matches
            || !execution_identities_match
            || on_chain_nonce.is_none()
            || post_state.is_none()
            || (original_present && fence_present)
        {
            RedemptionResolution::IntegrityFailure
        } else {
            match nonce_relation {
                Some(NonceRelation::Current)
                    if !original_present
                        && !fence_present
                        && post_state == Some(PostStateRelation::Unchanged) =>
                {
                    RedemptionResolution::Unresolved
                }
                Some(NonceRelation::Successor)
                    if original_present
                        && !fence_present
                        && post_state == Some(PostStateRelation::Redeemed) =>
                {
                    RedemptionResolution::RedemptionFinalized
                }
                Some(NonceRelation::Successor)
                    if fence_present
                        && fence_durably_authorized
                        && !original_present
                        && post_state == Some(PostStateRelation::Unchanged) =>
                {
                    RedemptionResolution::PermanentlyFencedNoEffect
                }
                _ => RedemptionResolution::IntegrityFailure,
            }
        };
        Ok(SourceBoundVerifiedOutcome::from_raw_verifier(resolution))
    }

    fn parse<'a, T: Deserialize<'a>>(
        &self,
        response: &'a BoundedWireResponse,
        credentials: &ResolvedRedemptionCredentials,
    ) -> Result<T, WireParseError> {
        serde_json::from_slice(response.bytes.as_slice())
            .map_err(|_| response.failure(WireFailureClass::Malformed, credentials))
    }
}

fn validate_execution_query(
    query: &ExecutionQueryWire<'_>,
    expected_query_id: &str,
    safe_address: [u8; ADDRESS_BYTES],
    safe_transaction_hash: [u8; WORD_BYTES],
    finalized_number: Option<[u8; WORD_BYTES]>,
    finalized_hash: Option<[u8; WORD_BYTES]>,
    required_confirmations: u64,
    max_execution_logs: usize,
) -> Option<bool> {
    if query.query_id != expected_query_id
        || decode_address(query.safe_address) != Some(safe_address)
        || parse_word(query.safe_transaction_hash) != Some(safe_transaction_hash)
        || parse_quantity_word(query.observed_at_block_number) != finalized_number
        || parse_word(query.observed_at_block_hash) != finalized_hash
    {
        return None;
    }
    let Some(receipt) = query.receipts.0.as_ref() else {
        return query.canonical_block.0.is_none().then_some(false);
    };
    let canonical = query.canonical_block.0.as_ref()?;
    if max_execution_logs == 0 {
        return None;
    }
    let receipt_block_number = parse_quantity_word(receipt.block_number)?;
    let receipt_block_hash = parse_word(receipt.block_hash)?;
    let receipt_transaction_hash = parse_word(receipt.transaction_hash)?;
    let receipt_transaction_index = parse_quantity_word(receipt.transaction_index)?;
    let finalized_number = finalized_number?;
    let log = &receipt.logs.0;
    let expected_topic = keccak256(b"ExecutionSuccess(bytes32,uint256)").0;
    let coordinates_match = receipt.status == "0x1"
        && receipt_block_number != [0; WORD_BYTES]
        && confirmed_at(
            receipt_block_number,
            finalized_number,
            required_confirmations,
        )
        && receipt_block_hash != [0; WORD_BYTES]
        && parse_quantity_word(canonical.block_number) == Some(receipt_block_number)
        && parse_word(canonical.block_hash) == Some(receipt_block_hash)
        && receipt_transaction_hash != [0; WORD_BYTES]
        && receipt_transaction_index != [u8::MAX; WORD_BYTES]
        && !log.removed
        && decode_address(log.address) == Some(safe_address)
        && parse_word(log.topics[0]) == Some(expected_topic)
        && execution_data_matches(log.data, safe_transaction_hash)
        && parse_quantity_word(log.block_number) == Some(receipt_block_number)
        && parse_word(log.block_hash) == Some(receipt_block_hash)
        && parse_word(log.transaction_hash) == Some(receipt_transaction_hash)
        && parse_quantity_word(log.transaction_index) == Some(receipt_transaction_index)
        && parse_quantity_word(log.log_index).is_some_and(|value| value != [u8::MAX; WORD_BYTES]);
    Some(coordinates_match)
}

fn execution_data_matches(value: &str, safe_transaction_hash: [u8; WORD_BYTES]) -> bool {
    let Some(encoded) = value.strip_prefix("0x") else {
        return false;
    };
    if encoded.len() != WORD_BYTES * 4 {
        return false;
    }
    let bytes = encoded.as_bytes();
    bytes.iter().all(|byte| hex_nibble(*byte).is_some())
        && bytes[..WORD_BYTES * 2]
            .chunks_exact(2)
            .zip(safe_transaction_hash)
            .all(|(pair, expected)| {
                let Some(high) = hex_nibble(pair[0]) else {
                    return false;
                };
                let Some(low) = hex_nibble(pair[1]) else {
                    return false;
                };
                (high << 4) | low == expected
            })
}

fn confirmed_at(
    receipt_block: [u8; WORD_BYTES],
    finalized_block: [u8; WORD_BYTES],
    required_confirmations: u64,
) -> bool {
    let mut minimum = receipt_block;
    let mut carry = required_confirmations.saturating_sub(1);
    for byte in minimum.iter_mut().rev() {
        let sum = u64::from(*byte) + (carry & 0xff);
        *byte = sum as u8;
        carry = (carry >> 8) + (sum >> 8);
    }
    carry == 0 && minimum <= finalized_block
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayerState {
    New,
    Executed,
    Mined,
    Invalid,
    Confirmed,
    Failed,
}

impl RelayerState {
    pub fn is_terminal_proof(self) -> bool {
        let _ = self;
        false
    }
}

pub struct RelayerObservation {
    transaction_id: Zeroizing<Box<[u8]>>,
    transaction_id_digest: [u8; WORD_BYTES],
    state: RelayerState,
    transaction_hash: Option<[u8; WORD_BYTES]>,
    key_version: u32,
}

impl RelayerObservation {
    pub fn state(&self) -> RelayerState {
        self.state
    }

    pub fn projection(&self) -> RedactedProjection {
        RedactedProjection {
            class: ProjectionClass::RelayerResponse,
            item_count: 1,
            byte_len: self.transaction_id.len() + self.transaction_hash.map_or(0, |_| WORD_BYTES),
            keyed_digest: self.transaction_id_digest,
            key_version: self.key_version,
        }
    }

    pub(super) fn transaction_id(&self) -> &[u8] {
        &self.transaction_id
    }
}

impl Drop for RelayerObservation {
    fn drop(&mut self) {
        self.transaction_id_digest.zeroize();
        self.transaction_hash.zeroize();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFailureClass {
    Transport,
    Http,
    Oversize,
    Malformed,
    WrongItemCount,
    IdentityMismatch,
    FieldTooLarge,
    UnknownState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireDiagnostic {
    pub class: WireFailureClass,
    pub http_status: Option<u16>,
    pub projection: RedactedProjection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireParseError {
    pub diagnostic: WireDiagnostic,
}

impl WireParseError {
    fn from_capped(error: CappedIoError, class: ProjectionClass) -> Self {
        match error {
            CappedIoError::Oversize(projection) => Self {
                diagnostic: WireDiagnostic {
                    class: WireFailureClass::Oversize,
                    http_status: None,
                    projection,
                },
            },
            _ => Self {
                diagnostic: WireDiagnostic {
                    class: WireFailureClass::Transport,
                    http_status: None,
                    projection: RedactedProjection {
                        class,
                        item_count: 0,
                        byte_len: 0,
                        keyed_digest: [0; WORD_BYTES],
                        key_version: 0,
                    },
                },
            },
        }
    }
}

impl fmt::Display for WireParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "redacted redemption wire failure: {:?}",
            self.diagnostic
        )
    }
}

impl std::error::Error for WireParseError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitWire<'a> {
    #[serde(rename = "transactionID", borrow)]
    transaction_id: &'a str,
    #[serde(borrow)]
    state: &'a str,
    #[serde(rename = "transactionHash", borrow)]
    transaction_hash: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionWire<'a> {
    #[serde(rename = "transactionID", borrow)]
    transaction_id: &'a str,
    #[serde(rename = "transactionHash", borrow)]
    transaction_hash: &'a str,
    #[serde(borrow)]
    from: &'a str,
    #[serde(borrow)]
    to: &'a str,
    #[serde(rename = "proxyAddress", borrow)]
    proxy_address: &'a str,
    #[serde(borrow)]
    data: &'a str,
    #[serde(borrow)]
    nonce: &'a str,
    #[serde(borrow)]
    value: &'a str,
    #[serde(borrow)]
    state: &'a str,
    #[serde(rename = "type", borrow)]
    transaction_type: &'a str,
    #[serde(borrow)]
    metadata: &'a str,
    #[serde(rename = "createdAt", borrow)]
    created_at: &'a str,
    #[serde(rename = "updatedAt", borrow)]
    updated_at: &'a str,
}

struct ExactOne<'a>(TransactionWire<'a>);

struct ExactOneVisitor;

impl<'de> Visitor<'de> for ExactOneVisitor {
    type Value = ExactOne<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("exactly one relayer transaction")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let first = sequence
            .next_element::<TransactionWire<'de>>()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;
        if sequence.next_element::<de::IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(2, &self));
        }
        Ok(ExactOne(first))
    }
}

impl<'de> Deserialize<'de> for ExactOne<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(ExactOneVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NonceCallWire<'a> {
    #[serde(rename = "queryId", borrow)]
    query_id: &'a str,
    #[serde(rename = "safeAddress", borrow)]
    safe_address: &'a str,
    #[serde(borrow)]
    calldata: &'a str,
    #[serde(rename = "blockNumber", borrow)]
    block_number: &'a str,
    #[serde(rename = "blockHash", borrow)]
    block_hash: &'a str,
    #[serde(borrow)]
    result: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutionQueryWire<'a> {
    #[serde(rename = "queryId", borrow)]
    query_id: &'a str,
    #[serde(rename = "safeAddress", borrow)]
    safe_address: &'a str,
    #[serde(rename = "safeTransactionHash", borrow)]
    safe_transaction_hash: &'a str,
    #[serde(rename = "observedAtBlockNumber", borrow)]
    observed_at_block_number: &'a str,
    #[serde(rename = "observedAtBlockHash", borrow)]
    observed_at_block_hash: &'a str,
    #[serde(borrow)]
    receipts: ZeroOrOneReceipt<'a>,
    #[serde(rename = "canonicalBlock", borrow)]
    canonical_block: RequiredCanonicalBlock<'a>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalBlockWire<'a> {
    #[serde(rename = "blockNumber", borrow)]
    block_number: &'a str,
    #[serde(rename = "blockHash", borrow)]
    block_hash: &'a str,
}

struct RequiredCanonicalBlock<'a>(Option<CanonicalBlockWire<'a>>);

struct RequiredCanonicalBlockVisitor;

impl<'de> Visitor<'de> for RequiredCanonicalBlockVisitor {
    type Value = RequiredCanonicalBlock<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a required nullable canonical receipt block")
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(RequiredCanonicalBlock(None))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_none()
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        CanonicalBlockWire::deserialize(deserializer)
            .map(Some)
            .map(RequiredCanonicalBlock)
    }
}

impl<'de> Deserialize<'de> for RequiredCanonicalBlock<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_option(RequiredCanonicalBlockVisitor)
    }
}

struct ZeroOrOneReceipt<'a>(Option<ReceiptWire<'a>>);

struct ZeroOrOneReceiptVisitor;

impl<'de> Visitor<'de> for ZeroOrOneReceiptVisitor {
    type Value = ZeroOrOneReceipt<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("zero or one exact execution receipt")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let first = sequence.next_element::<ReceiptWire<'de>>()?;
        if sequence.next_element::<de::IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(2, &self));
        }
        Ok(ZeroOrOneReceipt(first))
    }
}

impl<'de> Deserialize<'de> for ZeroOrOneReceipt<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(ZeroOrOneReceiptVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptWire<'a> {
    #[serde(rename = "transactionHash", borrow)]
    transaction_hash: &'a str,
    #[serde(rename = "blockNumber", borrow)]
    block_number: &'a str,
    #[serde(rename = "blockHash", borrow)]
    block_hash: &'a str,
    #[serde(rename = "transactionIndex", borrow)]
    transaction_index: &'a str,
    #[serde(borrow)]
    status: &'a str,
    #[serde(borrow)]
    logs: ExactOneExecutionLog<'a>,
}

struct ExactOneExecutionLog<'a>(ExecutionLogWire<'a>);

struct ExactOneExecutionLogVisitor;

impl<'de> Visitor<'de> for ExactOneExecutionLogVisitor {
    type Value = ExactOneExecutionLog<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("exactly one Safe execution log")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let first = sequence
            .next_element::<ExecutionLogWire<'de>>()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;
        if sequence.next_element::<de::IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(2, &self));
        }
        Ok(ExactOneExecutionLog(first))
    }
}

impl<'de> Deserialize<'de> for ExactOneExecutionLog<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(ExactOneExecutionLogVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionLogWire<'a> {
    #[serde(borrow)]
    address: &'a str,
    #[serde(borrow)]
    topics: [&'a str; 1],
    #[serde(borrow)]
    data: &'a str,
    #[serde(rename = "blockNumber", borrow)]
    block_number: &'a str,
    #[serde(rename = "blockHash", borrow)]
    block_hash: &'a str,
    #[serde(rename = "transactionHash", borrow)]
    transaction_hash: &'a str,
    #[serde(rename = "transactionIndex", borrow)]
    transaction_index: &'a str,
    #[serde(rename = "logIndex", borrow)]
    log_index: &'a str,
    removed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostStateWire<'a> {
    #[serde(rename = "queryId", borrow)]
    query_id: &'a str,
    #[serde(borrow)]
    target: &'a str,
    #[serde(rename = "conditionId", borrow)]
    condition_id: &'a str,
    #[serde(rename = "blockNumber", borrow)]
    block_number: &'a str,
    #[serde(rename = "blockHash", borrow)]
    block_hash: &'a str,
    #[serde(borrow)]
    results: [&'a str; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SafeBoundaryWire<'a> {
    #[serde(rename = "queryId", borrow)]
    query_id: &'a str,
    #[serde(borrow)]
    safe: &'a str,
    #[serde(borrow)]
    factory: &'a str,
    #[serde(borrow)]
    implementation: &'a str,
    #[serde(rename = "fallbackHandler", borrow)]
    fallback_handler: &'a str,
    #[serde(borrow)]
    guard: &'a str,
    #[serde(borrow)]
    modules: [&'a str; 0],
    #[serde(rename = "blockNumber", borrow)]
    block_number: &'a str,
    #[serde(rename = "blockHash", borrow)]
    block_hash: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FinalizedHeadWire<'a> {
    #[serde(rename = "queryId", borrow)]
    query_id: &'a str,
    #[serde(rename = "chainId")]
    chain_id: u64,
    #[serde(rename = "blockNumber", borrow)]
    block_number: &'a str,
    #[serde(rename = "blockHash", borrow)]
    block_hash: &'a str,
}

fn validate_id(value: &str, profile: &ValidatedRedemptionProfile) -> Result<(), WireFailureClass> {
    if value.is_empty() || value.len() > profile.max_transaction_id_bytes() {
        Err(WireFailureClass::FieldTooLarge)
    } else {
        Ok(())
    }
}

fn parse_state(value: &str) -> Option<RelayerState> {
    match value {
        "STATE_NEW" => Some(RelayerState::New),
        "STATE_EXECUTED" => Some(RelayerState::Executed),
        "STATE_MINED" => Some(RelayerState::Mined),
        "STATE_INVALID" => Some(RelayerState::Invalid),
        "STATE_CONFIRMED" => Some(RelayerState::Confirmed),
        "STATE_FAILED" => Some(RelayerState::Failed),
        _ => None,
    }
}

fn parse_optional_hash(value: &str) -> Option<Option<[u8; WORD_BYTES]>> {
    if value.is_empty() {
        Some(None)
    } else {
        parse_word(value).map(Some)
    }
}

fn parse_word(value: &str) -> Option<[u8; WORD_BYTES]> {
    decode_fixed(value)
}

fn decode_address(value: &str) -> Option<[u8; ADDRESS_BYTES]> {
    decode_fixed(value)
}

fn decode_fixed<const N: usize>(value: &str) -> Option<[u8; N]> {
    let encoded = value.strip_prefix("0x")?;
    let mut output = [0; N];
    hex::decode_to_slice(encoded, &mut output).ok()?;
    Some(output)
}

fn hex_matches(value: &str, expected: &[u8]) -> bool {
    let Some(encoded) = value.strip_prefix("0x") else {
        return false;
    };
    if encoded.len() != expected.len() * 2 {
        return false;
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .zip(expected)
        .all(|(pair, byte)| {
            let Some(high) = hex_nibble(pair[0]) else {
                return false;
            };
            let Some(low) = hex_nibble(pair[1]) else {
                return false;
            };
            (high << 4) | low == *byte
        })
}

fn parse_quantity_word(value: &str) -> Option<[u8; WORD_BYTES]> {
    let encoded = value.strip_prefix("0x")?;
    if encoded.is_empty()
        || encoded.len() > WORD_BYTES * 2
        || (encoded.len() > 1 && encoded.starts_with('0'))
    {
        return None;
    }
    let mut output = [0; WORD_BYTES];
    let mut source_index = encoded.len();
    let mut output_index = WORD_BYTES;
    while source_index > 0 {
        let low = hex_nibble(encoded.as_bytes()[source_index - 1])?;
        source_index -= 1;
        let high = if source_index > 0 {
            let value = hex_nibble(encoded.as_bytes()[source_index - 1])?;
            source_index -= 1;
            value
        } else {
            0
        };
        output_index -= 1;
        output[output_index] = (high << 4) | low;
    }
    Some(output)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn zeroizing_box(value: &[u8]) -> Zeroizing<Box<[u8]>> {
    let mut output = Zeroizing::new(vec![0; value.len()].into_boxed_slice());
    output.copy_from_slice(value);
    output
}
