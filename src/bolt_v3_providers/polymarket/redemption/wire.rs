use std::fmt;
#[cfg(test)]
use std::io::Cursor;

use serde::de::{self, DeserializeSeed, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::bounded::{
    CappedBytes, CappedIoError, ProjectionClass, RedactedProjection, keyed_digest,
};
use super::config::{ResolvedRedemptionCredentials, ValidatedRedemptionProfile};
use super::nonce::SafeNonce;
use super::query::{
    ExactQueryBinding, ExactQuerySet, ExpectedResponseClass, NonceRelation, PostStateRelation,
    QueryKind, RedemptionResolution, SourceBoundVerifiedOutcome, VerifiedOutcomeBinding,
    classify_nonce_successor,
};
use super::request::{
    FenceMayHaveStartedRequest, MarketMode, OriginalMayHaveStartedRequest, PreparedRequestPair,
    RequestKind,
};

const ADDRESS_BYTES: usize = 20;
const WORD_BYTES: usize = 32;

mod action_binding_private {
    pub trait Sealed {}
}

/// Sealed identity of the exact quorum-durable action under observation.
///
/// This is deliberately not response-source authority. Only an opaque
/// source-specific response capability can attest where response bytes came from.
pub trait ExactActionBinding: action_binding_private::Sealed {
    #[doc(hidden)]
    fn prepared_request_pair(&self) -> &PreparedRequestPair;
}

impl action_binding_private::Sealed for OriginalMayHaveStartedRequest {}
impl action_binding_private::Sealed for FenceMayHaveStartedRequest {}

impl ExactActionBinding for OriginalMayHaveStartedRequest {
    fn prepared_request_pair(&self) -> &PreparedRequestPair {
        self.prepared()
    }
}

impl ExactActionBinding for FenceMayHaveStartedRequest {
    fn prepared_request_pair(&self) -> &PreparedRequestPair {
        self.prepared()
    }
}

struct BoundedWireResponse {
    bytes: CappedBytes,
    class: ProjectionClass,
}

impl BoundedWireResponse {
    #[cfg(test)]
    fn from_hermetic_bytes(
        bytes: &[u8],
        limit: usize,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        class: ProjectionClass,
    ) -> Result<Self, WireParseError> {
        let bytes = CappedBytes::read_with_probe(
            Cursor::new(bytes),
            limit,
            profile.overflow_probe_bytes(),
            credentials.redaction_hmac_key(),
            credentials.key_version(),
            class,
        )
        .map_err(|error| WireParseError::from_capped(error, class))?;
        Ok(Self { bytes, class })
    }

    fn projection(&self, credentials: &ResolvedRedemptionCredentials) -> RedactedProjection {
        self.bytes.projection(
            self.class,
            1,
            credentials.redaction_hmac_key(),
            credentials.key_version(),
        )
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

fn context_failure(
    credentials: &ResolvedRedemptionCredentials,
    class: ProjectionClass,
) -> WireParseError {
    WireParseError {
        diagnostic: WireDiagnostic {
            class: WireFailureClass::IntegrityFailure,
            http_status: None,
            projection: RedactedProjection {
                class,
                item_count: 0,
                byte_len: 0,
                keyed_digest: keyed_digest(credentials.redaction_hmac_key(), &[]),
                key_version: credentials.key_version(),
            },
        },
    }
}

struct ActionSourceBinding {
    profile_digest: [u8; WORD_BYTES],
    config_digest: [u8; WORD_BYTES],
    key_version: u32,
    chain_id: u64,
    source_identity: [u8; WORD_BYTES],
    action_digest: [u8; WORD_BYTES],
    condition_id: [u8; WORD_BYTES],
    pre_claim_balances: [[u8; WORD_BYTES]; 2],
    pre_collateral_balance: [u8; WORD_BYTES],
    expected_redeemed_collateral_balance: [u8; WORD_BYTES],
    safe_nonce: SafeNonce,
    original_body_hash: [u8; WORD_BYTES],
    fence_body_hash: [u8; WORD_BYTES],
}

impl ActionSourceBinding {
    fn for_source(
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        prepared: &PreparedRequestPair,
        source_identity: [u8; WORD_BYTES],
    ) -> Option<Self> {
        if !prepared.matches_context(profile, credentials) {
            return None;
        }
        let (
            profile_digest,
            config_digest,
            relayer_source_identity,
            chain_source_identity,
            key_version,
        ) = prepared.context_identity();
        if source_identity != relayer_source_identity && source_identity != chain_source_identity {
            return None;
        }
        let [original_body_hash, fence_body_hash] = prepared.body_hashes();
        Some(Self {
            profile_digest,
            config_digest,
            key_version,
            chain_id: profile.chain_id(),
            source_identity,
            action_digest: prepared.action_digest(),
            condition_id: prepared.condition_id(),
            pre_claim_balances: prepared.pre_claim_balances(),
            pre_collateral_balance: prepared.pre_collateral_balance(),
            expected_redeemed_collateral_balance: prepared.expected_redeemed_collateral_balance(),
            safe_nonce: prepared.safe_nonce(),
            original_body_hash,
            fence_body_hash,
        })
    }

    fn matches(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        prepared: &PreparedRequestPair,
        source_identity: [u8; WORD_BYTES],
    ) -> bool {
        let Some(expected) = Self::for_source(profile, credentials, prepared, source_identity)
        else {
            return false;
        };
        self.profile_digest == expected.profile_digest
            && self.config_digest == expected.config_digest
            && self.key_version == expected.key_version
            && self.chain_id == expected.chain_id
            && self.source_identity == expected.source_identity
            && self.action_digest == expected.action_digest
            && self.condition_id == expected.condition_id
            && self.pre_claim_balances == expected.pre_claim_balances
            && self.pre_collateral_balance == expected.pre_collateral_balance
            && self.expected_redeemed_collateral_balance
                == expected.expected_redeemed_collateral_balance
            && self.safe_nonce == expected.safe_nonce
            && self.original_body_hash == expected.original_body_hash
            && self.fence_body_hash == expected.fence_body_hash
    }

    fn matches_context(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        source_identity: [u8; WORD_BYTES],
    ) -> bool {
        self.profile_digest == profile.profile_digest()
            && self.config_digest == profile.config_digest()
            && self.key_version == credentials.key_version()
            && self.chain_id == profile.chain_id()
            && self.source_identity == source_identity
    }
}

fn submit_response_binding(
    profile: &ValidatedRedemptionProfile,
    prepared: &PreparedRequestPair,
    kind: RequestKind,
) -> ExactQueryBinding {
    let request = prepared.request(kind);
    ExactQueryBinding {
        kind: match kind {
            RequestKind::Original => QueryKind::OriginalSubmit,
            RequestKind::Fence => QueryKind::FenceSubmit,
        },
        request_digest: request.body_hash(),
        path_digest: Sha256::digest(profile.submit_path().as_bytes()).into(),
        calldata_digest: Sha256::digest(request.calldata()).into(),
        response_class: ExpectedResponseClass::Submit,
    }
}

/// Opaque proof that bounded bytes were obtained from the configured relayer.
/// No production issuer exists in this mechanically disabled slice.
pub struct RelayerSourceResponse {
    response: BoundedWireResponse,
    binding: ActionSourceBinding,
    request_binding: ExactQueryBinding,
}

impl RelayerSourceResponse {
    #[cfg(test)]
    pub(super) fn from_hermetic_submit_bytes(
        authority: &impl ExactActionBinding,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        kind: RequestKind,
        bytes: &[u8],
    ) -> Result<Self, WireParseError> {
        let binding = ActionSourceBinding::for_source(
            profile,
            credentials,
            authority.prepared_request_pair(),
            profile.relayer_source_identity(),
        )
        .ok_or_else(|| context_failure(credentials, ProjectionClass::RelayerResponse))?;
        Ok(Self {
            response: BoundedWireResponse::from_hermetic_bytes(
                bytes,
                profile.max_relayer_response_bytes(),
                profile,
                credentials,
                ProjectionClass::RelayerResponse,
            )?,
            binding,
            request_binding: submit_response_binding(
                profile,
                authority.prepared_request_pair(),
                kind,
            ),
        })
    }

    #[cfg(test)]
    pub(super) fn from_hermetic_query_bytes(
        authority: &impl ExactActionBinding,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        queries: &ExactQuerySet,
        bytes: &[u8],
    ) -> Result<Self, WireParseError> {
        let binding = ActionSourceBinding::for_source(
            profile,
            credentials,
            authority.prepared_request_pair(),
            profile.relayer_source_identity(),
        )
        .ok_or_else(|| context_failure(credentials, ProjectionClass::RelayerResponse))?;
        Ok(Self {
            response: BoundedWireResponse::from_hermetic_bytes(
                bytes,
                profile.max_relayer_response_bytes(),
                profile,
                credentials,
                ProjectionClass::RelayerResponse,
            )?,
            binding,
            request_binding: queries
                .binding(QueryKind::RelayerTransaction)
                .map_err(|_| context_failure(credentials, ProjectionClass::RelayerResponse))?,
        })
    }

    pub fn projection(&self, credentials: &ResolvedRedemptionCredentials) -> RedactedProjection {
        self.response.projection(credentials)
    }

    #[cfg(test)]
    pub(super) fn with_hermetic_source_identity(
        mut self,
        source_identity: [u8; WORD_BYTES],
    ) -> Self {
        self.binding.source_identity = source_identity;
        self
    }

    pub fn parse_submit(
        &self,
        authority: &impl ExactActionBinding,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        kind: RequestKind,
    ) -> Result<RelayerObservation, WireParseError> {
        if !authority
            .prepared_request_pair()
            .matches_context(profile, credentials)
            || !self.binding.matches_context(
                profile,
                credentials,
                profile.relayer_source_identity(),
            )
        {
            return Err(context_failure(
                credentials,
                ProjectionClass::RelayerResponse,
            ));
        }
        if !self.binding.matches(
            profile,
            credentials,
            authority.prepared_request_pair(),
            profile.relayer_source_identity(),
        ) {
            return Err(self
                .response
                .failure(WireFailureClass::IdentityMismatch, credentials));
        }
        if self.request_binding
            != submit_response_binding(profile, authority.prepared_request_pair(), kind)
        {
            return Err(self
                .response
                .failure(WireFailureClass::IntegrityFailure, credentials));
        }
        let parsed: SubmitWire<'_> = serde_json::from_slice(self.response.bytes.as_slice())
            .map_err(|_| {
                self.response
                    .failure(WireFailureClass::Malformed, credentials)
            })?;
        validate_id(parsed.transaction_id, profile)
            .map_err(|class| self.response.failure(class, credentials))?;
        let state = parse_state(parsed.state).ok_or_else(|| {
            self.response
                .failure(WireFailureClass::UnknownState, credentials)
        })?;
        let transaction_hash = parse_optional_hash(parsed.transaction_hash).ok_or_else(|| {
            self.response
                .failure(WireFailureClass::Malformed, credentials)
        })?;
        Ok(RelayerObservation {
            transaction_id: zeroizing_vec(parsed.transaction_id.as_bytes()).map_err(|_| {
                self.response
                    .failure(WireFailureClass::Capacity, credentials)
            })?,
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
        authority: &impl ExactActionBinding,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        queries: &ExactQuerySet,
        expected: &RelayerObservation,
        kind: RequestKind,
    ) -> Result<RelayerObservation, WireParseError> {
        if !authority
            .prepared_request_pair()
            .matches_context(profile, credentials)
            || !self.binding.matches_context(
                profile,
                credentials,
                profile.relayer_source_identity(),
            )
        {
            return Err(context_failure(
                credentials,
                ProjectionClass::RelayerResponse,
            ));
        }
        if !self.binding.matches(
            profile,
            credentials,
            authority.prepared_request_pair(),
            profile.relayer_source_identity(),
        ) {
            return Err(self
                .response
                .failure(WireFailureClass::IdentityMismatch, credentials));
        }
        if queries.binding(QueryKind::RelayerTransaction).ok() != Some(self.request_binding) {
            return Err(self
                .response
                .failure(WireFailureClass::IntegrityFailure, credentials));
        }
        let exact: ExactOne<'_> =
            serde_json::from_slice(self.response.bytes.as_slice()).map_err(|_| {
                self.response
                    .failure(WireFailureClass::Malformed, credentials)
            })?;
        let transaction = exact.0;
        validate_id(transaction.transaction_id, profile)
            .map_err(|class| self.response.failure(class, credentials))?;
        if transaction.transaction_id.as_bytes() != expected.transaction_id.as_ref() {
            return Err(self
                .response
                .failure(WireFailureClass::IdentityMismatch, credentials));
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
            return Err(self
                .response
                .failure(WireFailureClass::IdentityMismatch, credentials));
        }
        if transaction.created_at.len() > profile.max_timestamp_bytes()
            || transaction.updated_at.len() > profile.max_timestamp_bytes()
            || transaction.metadata.len() > profile.max_metadata_bytes()
        {
            return Err(self
                .response
                .failure(WireFailureClass::FieldTooLarge, credentials));
        }
        let state = parse_state(transaction.state).ok_or_else(|| {
            self.response
                .failure(WireFailureClass::UnknownState, credentials)
        })?;
        let transaction_hash =
            parse_optional_hash(transaction.transaction_hash).ok_or_else(|| {
                self.response
                    .failure(WireFailureClass::Malformed, credentials)
            })?;
        if transaction_hash.is_some_and(|hash| hash != request.identity().safe_transaction_hash())
            || expected
                .transaction_hash
                .is_some_and(|expected_hash| Some(expected_hash) != transaction_hash)
        {
            return Err(self
                .response
                .failure(WireFailureClass::IdentityMismatch, credentials));
        }
        Ok(RelayerObservation {
            transaction_id: zeroizing_vec(transaction.transaction_id.as_bytes()).map_err(|_| {
                self.response
                    .failure(WireFailureClass::Capacity, credentials)
            })?,
            transaction_id_digest: expected.transaction_id_digest,
            state,
            transaction_hash,
            key_version: expected.key_version,
        })
    }
}

/// Opaque proof that bounded bytes were obtained at one exact finalized-chain
/// coordinate from the configured chain source. There is no production issuer.
pub struct FinalizedChainSourceResponse {
    response: BoundedWireResponse,
    binding: ActionSourceBinding,
    finalized_block_number: [u8; WORD_BYTES],
    finalized_block_hash: [u8; WORD_BYTES],
    request_binding: ExactQueryBinding,
}

impl FinalizedChainSourceResponse {
    #[cfg(test)]
    pub(super) fn from_hermetic_bytes(
        authority: &impl ExactActionBinding,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        queries: &ExactQuerySet,
        kind: QueryKind,
        finalized_block_number: [u8; WORD_BYTES],
        finalized_block_hash: [u8; WORD_BYTES],
        bytes: &[u8],
    ) -> Result<Self, WireParseError> {
        let binding = ActionSourceBinding::for_source(
            profile,
            credentials,
            authority.prepared_request_pair(),
            profile.chain_source_identity(),
        )
        .ok_or_else(|| context_failure(credentials, ProjectionClass::ChainResponse))?;
        Ok(Self {
            response: BoundedWireResponse::from_hermetic_bytes(
                bytes,
                profile.max_rpc_response_bytes(),
                profile,
                credentials,
                ProjectionClass::ChainResponse,
            )?,
            binding,
            finalized_block_number,
            finalized_block_hash,
            request_binding: queries
                .binding(kind)
                .map_err(|_| context_failure(credentials, ProjectionClass::ChainResponse))?,
        })
    }

    pub fn projection(&self, credentials: &ResolvedRedemptionCredentials) -> RedactedProjection {
        self.response.projection(credentials)
    }

    fn same_binding(&self, other: &Self) -> bool {
        self.binding.profile_digest == other.binding.profile_digest
            && self.binding.config_digest == other.binding.config_digest
            && self.binding.key_version == other.binding.key_version
            && self.binding.chain_id == other.binding.chain_id
            && self.binding.source_identity == other.binding.source_identity
            && self.binding.action_digest == other.binding.action_digest
            && self.binding.condition_id == other.binding.condition_id
            && self.binding.pre_claim_balances == other.binding.pre_claim_balances
            && self.binding.pre_collateral_balance == other.binding.pre_collateral_balance
            && self.binding.expected_redeemed_collateral_balance
                == other.binding.expected_redeemed_collateral_balance
            && self.binding.safe_nonce == other.binding.safe_nonce
            && self.binding.original_body_hash == other.binding.original_body_hash
            && self.binding.fence_body_hash == other.binding.fence_body_hash
            && self.finalized_block_number == other.finalized_block_number
            && self.finalized_block_hash == other.finalized_block_hash
    }

    fn matches_query(&self, queries: &ExactQuerySet, kind: QueryKind) -> bool {
        queries.binding(kind).ok() == Some(self.request_binding)
            && self.request_binding.kind == kind
    }

    fn matches_action(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        prepared: &PreparedRequestPair,
    ) -> bool {
        self.binding.matches(
            profile,
            credentials,
            prepared,
            profile.chain_source_identity(),
        ) && self.finalized_block_number != [0; WORD_BYTES]
            && self.finalized_block_hash != [0; WORD_BYTES]
    }
}

pub struct ExactQueryResponses {
    nonce: FinalizedChainSourceResponse,
    original_execution: FinalizedChainSourceResponse,
    fence_execution: FinalizedChainSourceResponse,
    post_state: FinalizedChainSourceResponse,
    safe_boundary: FinalizedChainSourceResponse,
    finalized_head: FinalizedChainSourceResponse,
}

impl ExactQueryResponses {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        queries: &ExactQuerySet,
        credentials: &ResolvedRedemptionCredentials,
        nonce: FinalizedChainSourceResponse,
        original_execution: FinalizedChainSourceResponse,
        fence_execution: FinalizedChainSourceResponse,
        post_state: FinalizedChainSourceResponse,
        safe_boundary: FinalizedChainSourceResponse,
        finalized_head: FinalizedChainSourceResponse,
    ) -> Result<Self, WireParseError> {
        for (response, kind) in [
            (&nonce, QueryKind::SafeNonce),
            (&original_execution, QueryKind::OriginalFinalizedReceiptLogs),
            (&fence_execution, QueryKind::FenceFinalizedReceiptLogs),
            (&post_state, QueryKind::RawPostState),
            (&safe_boundary, QueryKind::SafeBoundary),
            (&finalized_head, QueryKind::FinalizedHead),
        ] {
            if !nonce.same_binding(response) || !response.matches_query(queries, kind) {
                return Err(response
                    .response
                    .failure(WireFailureClass::IntegrityFailure, credentials));
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

    pub(super) fn matches_terminal_binding(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        prepared: &PreparedRequestPair,
        finalized_block_number: [u8; WORD_BYTES],
        finalized_block_hash: [u8; WORD_BYTES],
    ) -> bool {
        self.finalized_head
            .matches_action(profile, credentials, prepared)
            && self.finalized_head.finalized_block_number == finalized_block_number
            && self.finalized_head.finalized_block_hash == finalized_block_hash
    }

    #[cfg(test)]
    pub(super) fn with_hermetic_finalized_coordinates(
        mut self,
        finalized_block_number: [u8; WORD_BYTES],
        finalized_block_hash: [u8; WORD_BYTES],
    ) -> Self {
        for response in [
            &mut self.nonce,
            &mut self.original_execution,
            &mut self.fence_execution,
            &mut self.post_state,
            &mut self.safe_boundary,
            &mut self.finalized_head,
        ] {
            response.finalized_block_number = finalized_block_number;
            response.finalized_block_hash = finalized_block_hash;
        }
        self
    }

    #[cfg(test)]
    pub(super) fn with_hermetic_chain_source_identity(
        mut self,
        source_identity: [u8; WORD_BYTES],
    ) -> Self {
        for response in [
            &mut self.nonce,
            &mut self.original_execution,
            &mut self.fence_execution,
            &mut self.post_state,
            &mut self.safe_boundary,
            &mut self.finalized_head,
        ] {
            response.binding.source_identity = source_identity;
        }
        self
    }

    pub fn verify_after_fence(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        attempt: &FenceMayHaveStartedRequest,
    ) -> Result<SourceBoundVerifiedOutcome, WireParseError> {
        self.verify(profile, credentials, attempt.prepared(), true)
    }

    pub(super) fn matches_context_binding(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
    ) -> bool {
        [
            &self.nonce,
            &self.original_execution,
            &self.fence_execution,
            &self.post_state,
            &self.safe_boundary,
            &self.finalized_head,
        ]
        .iter()
        .all(|response| {
            response
                .binding
                .matches_context(profile, credentials, profile.chain_source_identity())
        })
    }

    fn verify(
        &self,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        prepared: &PreparedRequestPair,
        fence_durably_authorized: bool,
    ) -> Result<SourceBoundVerifiedOutcome, WireParseError> {
        if !prepared.matches_context(profile, credentials) {
            return Err(context_failure(credentials, ProjectionClass::ChainResponse));
        }
        for response in [
            &self.nonce,
            &self.original_execution,
            &self.fence_execution,
            &self.post_state,
            &self.safe_boundary,
            &self.finalized_head,
        ] {
            if !response.binding.matches_context(
                profile,
                credentials,
                profile.chain_source_identity(),
            ) {
                return Err(context_failure(credentials, ProjectionClass::ChainResponse));
            }
            if !response.matches_action(profile, credentials, prepared) {
                return Err(response
                    .response
                    .failure(WireFailureClass::IdentityMismatch, credentials));
            }
        }
        let nonce: NonceCallWire<'_> = self.parse(&self.nonce, credentials)?;
        let original: ExecutionQueryWire<'_> =
            match self.parse(&self.original_execution, credentials) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(integrity_outcome(
                        profile,
                        prepared,
                        self,
                        fence_durably_authorized,
                    ));
                }
            };
        let fence: ExecutionQueryWire<'_> = match self.parse(&self.fence_execution, credentials) {
            Ok(value) => value,
            Err(_) => {
                return Ok(integrity_outcome(
                    profile,
                    prepared,
                    self,
                    fence_durably_authorized,
                ));
            }
        };
        let post: PostStateWire<'_> = self.parse(&self.post_state, credentials)?;
        let boundary: SafeBoundaryWire<'_> = self.parse(&self.safe_boundary, credentials)?;
        let finalized: FinalizedHeadWire<'_> = self.parse(&self.finalized_head, credentials)?;

        let finalized_number = parse_quantity_word(finalized.block_number);
        let finalized_hash = parse_word(finalized.block_hash);
        if finalized_number != Some(self.finalized_head.finalized_block_number)
            || finalized_hash != Some(self.finalized_head.finalized_block_hash)
        {
            return Err(self
                .finalized_head
                .response
                .failure(WireFailureClass::IdentityMismatch, credentials));
        }
        let nonce_number = parse_quantity_word(nonce.block_number);
        let nonce_hash = parse_word(nonce.block_hash);
        let post_number = parse_quantity_word(post.block_number);
        let post_hash = parse_word(post.block_hash);
        let boundary_number = parse_quantity_word(boundary.block_number);
        let boundary_hash = parse_word(boundary.block_hash);
        let on_chain_nonce = parse_word(nonce.result).map(SafeNonce::from_be_bytes);
        let post_claim_balances = [
            parse_word(post.claim_results[0]),
            parse_word(post.claim_results[1]),
        ];
        let post_collateral_balance = parse_word(post.collateral_balance);
        let (collateral, output_asset) = profile.post_state_assets();
        let (factory, implementation, fallback_handler, guard) = profile.safe_boundary();
        let common_identity_matches = finalized.query_id == "finalized_head"
            && finalized.chain_id == profile.chain_id()
            && finalized_number.is_some_and(|value| value != [0; WORD_BYTES])
            && finalized_hash.is_some_and(|value| value != [0; WORD_BYTES])
            && finalized_number == Some(self.finalized_head.finalized_block_number)
            && finalized_hash == Some(self.finalized_head.finalized_block_hash)
            && nonce.query_id == "safe_nonce"
            && decode_address(nonce.safe_address) == Some(profile.safe_address())
            && hex_matches(nonce.calldata, &profile.nonce_selector())
            && nonce_number == finalized_number
            && nonce_hash == finalized_hash
            && post.query_id == "raw_post_state"
            && decode_address(post.target)
                == Some(prepared.request(RequestKind::Original).identity().target())
            && parse_word(post.condition_id) == Some(prepared.condition_id())
            && decode_address(post.collateral) == Some(collateral)
            && decode_address(post.output_asset) == Some(output_asset)
            && decode_address(post.account) == Some(profile.safe_address())
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
            profile,
            prepared,
            RequestKind::Original,
            finalized_number,
            finalized_hash,
            profile.finality_confirmations(),
            profile.max_receipt_logs(),
        );
        let fence_receipt = validate_execution_query(
            &fence,
            "fence_finalized_receipt_logs",
            profile,
            prepared,
            RequestKind::Fence,
            finalized_number,
            finalized_hash,
            profile.finality_confirmations(),
            profile.max_receipt_logs(),
        );
        let execution_identities_match = original_receipt != ReceiptCompatibility::Invalid
            && fence_receipt != ReceiptCompatibility::Invalid;
        let original_present = original_receipt == ReceiptCompatibility::Compatible;
        let fence_present = fence_receipt == ReceiptCompatibility::Compatible;
        let post_state = match (post_claim_balances, post_collateral_balance) {
            ([Some(first), Some(second)], Some(collateral_balance))
                if [first, second] == [[0; WORD_BYTES]; 2]
                    && collateral_balance == prepared.expected_redeemed_collateral_balance() =>
            {
                Some(PostStateRelation::Redeemed)
            }
            ([Some(first), Some(second)], Some(collateral_balance))
                if [first, second] == prepared.pre_claim_balances()
                    && collateral_balance == prepared.pre_collateral_balance() =>
            {
                Some(PostStateRelation::Unchanged)
            }
            ([Some(_), Some(_)], Some(_)) => Some(PostStateRelation::Drifted),
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
        Ok(SourceBoundVerifiedOutcome::from_raw_verifier(
            resolution,
            profile,
            prepared,
            VerifiedOutcomeBinding {
                finalized_block_number: self.finalized_head.finalized_block_number,
                finalized_block_hash: self.finalized_head.finalized_block_hash,
                fence_authorized: fence_durably_authorized,
            },
        ))
    }

    fn parse<'a, T: Deserialize<'a>>(
        &self,
        response: &'a FinalizedChainSourceResponse,
        credentials: &ResolvedRedemptionCredentials,
    ) -> Result<T, WireParseError> {
        serde_json::from_slice(response.response.bytes.as_slice()).map_err(|_| {
            response
                .response
                .failure(WireFailureClass::Malformed, credentials)
        })
    }
}

fn integrity_outcome(
    profile: &ValidatedRedemptionProfile,
    prepared: &PreparedRequestPair,
    responses: &ExactQueryResponses,
    fence_authorized: bool,
) -> SourceBoundVerifiedOutcome {
    SourceBoundVerifiedOutcome::from_raw_verifier(
        RedemptionResolution::IntegrityFailure,
        profile,
        prepared,
        VerifiedOutcomeBinding {
            finalized_block_number: responses.finalized_head.finalized_block_number,
            finalized_block_hash: responses.finalized_head.finalized_block_hash,
            fence_authorized,
        },
    )
}

fn validate_execution_query(
    query: &ExecutionQueryWire<'_>,
    expected_query_id: &str,
    profile: &ValidatedRedemptionProfile,
    prepared: &PreparedRequestPair,
    request_kind: RequestKind,
    finalized_number: Option<[u8; WORD_BYTES]>,
    finalized_hash: Option<[u8; WORD_BYTES]>,
    required_confirmations: u64,
    max_execution_logs: usize,
) -> ReceiptCompatibility {
    let safe_address = profile.safe_address();
    let request = prepared.request(request_kind);
    let safe_transaction_hash = request.identity().safe_transaction_hash();
    if query.query_id != expected_query_id
        || decode_address(query.safe_address) != Some(safe_address)
        || parse_word(query.safe_transaction_hash) != Some(safe_transaction_hash)
        || parse_quantity_word(query.observed_at_block_number) != finalized_number
        || parse_word(query.observed_at_block_hash) != finalized_hash
    {
        return ReceiptCompatibility::Invalid;
    }
    let Some(receipt) = query.receipts.0.as_ref() else {
        return if query.canonical_block.0.is_none() {
            ReceiptCompatibility::Absent
        } else {
            ReceiptCompatibility::Invalid
        };
    };
    let Some(canonical) = query.canonical_block.0.as_ref() else {
        return ReceiptCompatibility::Invalid;
    };
    if max_execution_logs == 0 {
        return ReceiptCompatibility::Invalid;
    }
    let Some(receipt_block_number) = parse_quantity_word(receipt.block_number) else {
        return ReceiptCompatibility::Invalid;
    };
    let Some(receipt_block_hash) = parse_word(receipt.block_hash) else {
        return ReceiptCompatibility::Invalid;
    };
    let Some(receipt_transaction_hash) = parse_word(receipt.transaction_hash) else {
        return ReceiptCompatibility::Invalid;
    };
    let Some(receipt_transaction_index) = parse_quantity_word(receipt.transaction_index) else {
        return ReceiptCompatibility::Invalid;
    };
    let Some(finalized_number) = finalized_number else {
        return ReceiptCompatibility::Invalid;
    };
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
        && receipt_transaction_index != [u8::MAX; WORD_BYTES];
    if !coordinates_match {
        return ReceiptCompatibility::Invalid;
    }
    let expected = ExpectedExecutionLogs {
        profile,
        prepared,
        request_kind,
        safe_transaction_hash,
        receipt_block_number,
        receipt_block_hash,
        receipt_transaction_hash,
        receipt_transaction_index,
        max_execution_logs,
    };
    let mut deserializer = serde_json::Deserializer::from_str(receipt.logs.get());
    let summary = ExecutionLogsSeed { expected }
        .deserialize(&mut deserializer)
        .ok()
        .filter(|_| deserializer.end().is_ok());
    match summary {
        Some(summary)
            if !summary.invalid
                && summary.safe_successes == 1
                && ((request_kind == RequestKind::Original && summary.payouts == 1)
                    || (request_kind == RequestKind::Fence && summary.payouts == 0)) =>
        {
            ReceiptCompatibility::Compatible
        }
        _ => ReceiptCompatibility::Invalid,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiptCompatibility {
    Absent,
    Compatible,
    Invalid,
}

#[derive(Clone, Copy)]
struct ExpectedExecutionLogs<'a> {
    profile: &'a ValidatedRedemptionProfile,
    prepared: &'a PreparedRequestPair,
    request_kind: RequestKind,
    safe_transaction_hash: [u8; WORD_BYTES],
    receipt_block_number: [u8; WORD_BYTES],
    receipt_block_hash: [u8; WORD_BYTES],
    receipt_transaction_hash: [u8; WORD_BYTES],
    receipt_transaction_index: [u8; WORD_BYTES],
    max_execution_logs: usize,
}

struct ExecutionLogsSeed<'a> {
    expected: ExpectedExecutionLogs<'a>,
}

struct ExecutionLogsVisitor<'a> {
    expected: ExpectedExecutionLogs<'a>,
}

#[derive(Default)]
struct ExecutionLogSummary {
    safe_successes: usize,
    payouts: usize,
    invalid: bool,
}

impl<'de> DeserializeSeed<'de> for ExecutionLogsSeed<'_> {
    type Value = ExecutionLogSummary;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(ExecutionLogsVisitor {
            expected: self.expected,
        })
    }
}

impl<'de> Visitor<'de> for ExecutionLogsVisitor<'_> {
    type Value = ExecutionLogSummary;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded complete execution-log set")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut summary = ExecutionLogSummary::default();
        let mut count = 0usize;
        while let Some(log) = sequence.next_element::<ExecutionLogWire<'de>>()? {
            count = count
                .checked_add(1)
                .ok_or_else(|| de::Error::custom("execution log count overflow"))?;
            if count > self.expected.max_execution_logs {
                return Err(de::Error::invalid_length(count, &self));
            }
            classify_execution_log(&log, self.expected, &mut summary);
        }
        Ok(summary)
    }
}

fn classify_execution_log(
    log: &ExecutionLogWire<'_>,
    expected: ExpectedExecutionLogs<'_>,
    summary: &mut ExecutionLogSummary,
) {
    let coordinates_match = !log.removed
        && parse_quantity_word(log.block_number) == Some(expected.receipt_block_number)
        && parse_word(log.block_hash) == Some(expected.receipt_block_hash)
        && parse_word(log.transaction_hash) == Some(expected.receipt_transaction_hash)
        && parse_quantity_word(log.transaction_index) == Some(expected.receipt_transaction_index)
        && parse_quantity_word(log.log_index).is_some_and(|value| value != [u8::MAX; WORD_BYTES]);
    let structurally_valid = decode_address(log.address).is_some()
        && log.topics.len > 0
        && log.topics.words[..log.topics.len]
            .iter()
            .all(|topic| topic.is_some_and(|value| parse_word(value).is_some()))
        && valid_hex_data(log.data);
    if !coordinates_match || !structurally_valid {
        summary.invalid = true;
        return;
    }
    let Some(topic0) = log.topics.words[0].and_then(parse_word) else {
        summary.invalid = true;
        return;
    };
    let (
        standard_emitter,
        negative_emitter,
        underlying_collateral,
        safe_topic,
        standard_topic,
        negative_topic,
    ) = expected.profile.terminal_event_contract();
    if topic0 == safe_topic {
        if decode_address(log.address) != Some(expected.profile.safe_address())
            || log.topics.len != 1
            || !execution_data_matches(log.data, expected.safe_transaction_hash)
        {
            summary.invalid = true;
        } else {
            summary.safe_successes += 1;
            if summary.safe_successes > 1 {
                summary.invalid = true;
            }
        }
        return;
    }
    if topic0 != standard_topic && topic0 != negative_topic {
        return;
    }
    if expected.request_kind == RequestKind::Fence {
        summary.invalid = true;
        return;
    }
    let request = expected.prepared.request(RequestKind::Original);
    let valid = match expected.prepared.mode() {
        MarketMode::Standard => {
            topic0 == standard_topic
                && decode_address(log.address) == Some(standard_emitter)
                && standard_payout_matches(
                    log,
                    request.identity().target(),
                    underlying_collateral,
                    expected.prepared,
                    expected.profile.adapter_arguments().2,
                )
        }
        MarketMode::NegativeRisk => {
            topic0 == negative_topic
                && decode_address(log.address) == Some(negative_emitter)
                && negative_risk_payout_matches(log, request.identity().target(), expected.prepared)
        }
    };
    if valid {
        summary.payouts += 1;
        if summary.payouts > 1 {
            summary.invalid = true;
        }
    } else {
        summary.invalid = true;
    }
}

fn standard_payout_matches(
    log: &ExecutionLogWire<'_>,
    redeemer: [u8; ADDRESS_BYTES],
    collateral: [u8; ADDRESS_BYTES],
    prepared: &PreparedRequestPair,
    index_sets: [u64; 2],
) -> bool {
    event_data_has_words(log.data, 6)
        && log.topics.len == 4
        && log.topics.words[1].and_then(parse_topic_address) == Some(redeemer)
        && log.topics.words[2].and_then(parse_topic_address) == Some(collateral)
        && log.topics.words[3].and_then(parse_word) == Some([0; WORD_BYTES])
        && event_data_word(log.data, 0) == Some(prepared.condition_id())
        && event_data_word(log.data, 1) == Some(small_word(96))
        && event_data_word(log.data, 2) == redeemed_amount(prepared)
        && event_data_word(log.data, 3) == Some(small_word(2))
        && event_data_word(log.data, 4) == Some(small_word(index_sets[0]))
        && event_data_word(log.data, 5) == Some(small_word(index_sets[1]))
}

fn negative_risk_payout_matches(
    log: &ExecutionLogWire<'_>,
    redeemer: [u8; ADDRESS_BYTES],
    prepared: &PreparedRequestPair,
) -> bool {
    event_data_has_words(log.data, 5)
        && log.topics.len == 3
        && log.topics.words[1].and_then(parse_topic_address) == Some(redeemer)
        && log.topics.words[2].and_then(parse_word) == Some(prepared.condition_id())
        && event_data_word(log.data, 0) == Some(small_word(64))
        && event_data_word(log.data, 1) == redeemed_amount(prepared)
        && event_data_word(log.data, 2) == Some(small_word(2))
        && event_data_word(log.data, 3) == Some(prepared.pre_claim_balances()[0])
        && event_data_word(log.data, 4) == Some(prepared.pre_claim_balances()[1])
}

fn redeemed_amount(prepared: &PreparedRequestPair) -> Option<[u8; WORD_BYTES]> {
    subtract_word(
        prepared.expected_redeemed_collateral_balance(),
        prepared.pre_collateral_balance(),
    )
}

pub(super) fn subtract_word(
    mut minuend: [u8; WORD_BYTES],
    subtrahend: [u8; WORD_BYTES],
) -> Option<[u8; WORD_BYTES]> {
    let mut borrow = 0u16;
    for index in (0..WORD_BYTES).rev() {
        let left = u16::from(minuend[index]);
        let right = u16::from(subtrahend[index]) + borrow;
        if left >= right {
            minuend[index] = (left - right) as u8;
            borrow = 0;
        } else {
            minuend[index] = (left + 256 - right) as u8;
            borrow = 1;
        }
    }
    (borrow == 0).then_some(minuend)
}

pub(super) fn small_word(value: u64) -> [u8; WORD_BYTES] {
    let mut word = [0; WORD_BYTES];
    word[WORD_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
    word
}

fn parse_topic_address(value: &str) -> Option<[u8; ADDRESS_BYTES]> {
    let word = parse_word(value)?;
    if word[..WORD_BYTES - ADDRESS_BYTES] != [0; WORD_BYTES - ADDRESS_BYTES] {
        return None;
    }
    word[WORD_BYTES - ADDRESS_BYTES..].try_into().ok()
}

fn event_data_word(value: &str, index: usize) -> Option<[u8; WORD_BYTES]> {
    let encoded = value.strip_prefix("0x")?;
    let start = index.checked_mul(WORD_BYTES * 2)?;
    let end = start.checked_add(WORD_BYTES * 2)?;
    let word = encoded.get(start..end)?;
    let mut output = [0; WORD_BYTES];
    for (slot, pair) in output.iter_mut().zip(word.as_bytes().chunks_exact(2)) {
        *slot = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(output)
}

fn event_data_has_words(value: &str, words: usize) -> bool {
    value.strip_prefix("0x").and_then(|encoded| {
        words
            .checked_mul(WORD_BYTES * 2)
            .map(|len| encoded.len() == len)
    }) == Some(true)
}

fn valid_hex_data(value: &str) -> bool {
    let Some(encoded) = value.strip_prefix("0x") else {
        return false;
    };
    encoded.len() % 2 == 0
        && encoded
            .as_bytes()
            .iter()
            .all(|byte| hex_nibble(*byte).is_some())
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
    transaction_id: Zeroizing<Vec<u8>>,
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
    Capacity,
    Malformed,
    WrongItemCount,
    IdentityMismatch,
    IntegrityFailure,
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
            CappedIoError::Allocation => Self {
                diagnostic: WireDiagnostic {
                    class: WireFailureClass::Capacity,
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
    logs: &'a RawValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionLogWire<'a> {
    #[serde(borrow)]
    address: &'a str,
    #[serde(borrow)]
    topics: TopicSet<'a>,
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

struct TopicSet<'a> {
    words: [Option<&'a str>; 4],
    len: usize,
}

struct TopicSetVisitor;

impl<'de> Visitor<'de> for TopicSetVisitor {
    type Value = TopicSet<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EVM event topic set")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut words = [None; 4];
        let mut len = 0;
        while let Some(word) = sequence.next_element::<&'de str>()? {
            if len == words.len() {
                return Err(de::Error::invalid_length(len + 1, &self));
            }
            words[len] = Some(word);
            len += 1;
        }
        Ok(TopicSet { words, len })
    }
}

impl<'de> Deserialize<'de> for TopicSet<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(TopicSetVisitor)
    }
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
    #[serde(borrow)]
    collateral: &'a str,
    #[serde(rename = "outputAsset", borrow)]
    output_asset: &'a str,
    #[serde(borrow)]
    account: &'a str,
    #[serde(rename = "blockNumber", borrow)]
    block_number: &'a str,
    #[serde(rename = "blockHash", borrow)]
    block_hash: &'a str,
    #[serde(rename = "claimResults", borrow)]
    claim_results: [&'a str; 2],
    #[serde(rename = "collateralBalance", borrow)]
    collateral_balance: &'a str,
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

fn zeroizing_vec(value: &[u8]) -> Result<Zeroizing<Vec<u8>>, ()> {
    let mut storage = Vec::new();
    storage.try_reserve_exact(value.len()).map_err(|_| ())?;
    storage.resize(value.len(), 0);
    let mut output = Zeroizing::new(storage);
    output.copy_from_slice(value);
    Ok(output)
}
