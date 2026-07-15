use super::bounded::{CappedBytes, ProjectionClass, RedactedProjection};
use super::config::{ResolvedRedemptionCredentials, ValidatedRedemptionProfile};
use super::nonce::SafeNonce;
use super::request::{
    FenceMayHaveStartedRequest, OriginalMayHaveStartedRequest, PreparedRequestPair, RequestKind,
};
use super::wire::{ExactQueryResponses, RelayerObservation};
use sha2::{Digest, Sha256};

const WORD_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    OriginalSubmit,
    FenceSubmit,
    RelayerTransaction,
    SafeNonce,
    OriginalFinalizedReceiptLogs,
    FenceFinalizedReceiptLogs,
    FinalizedHead,
    RawPostState,
    SafeBoundary,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ExpectedResponseClass {
    Submit,
    RelayerTransaction,
    SafeNonce,
    ExecutionReceipt,
    PostState,
    SafeBoundary,
    FinalizedHead,
}

impl ExpectedResponseClass {
    fn for_kind(kind: QueryKind) -> Self {
        match kind {
            QueryKind::OriginalSubmit | QueryKind::FenceSubmit => Self::Submit,
            QueryKind::RelayerTransaction => Self::RelayerTransaction,
            QueryKind::SafeNonce => Self::SafeNonce,
            QueryKind::OriginalFinalizedReceiptLogs | QueryKind::FenceFinalizedReceiptLogs => {
                Self::ExecutionReceipt
            }
            QueryKind::RawPostState => Self::PostState,
            QueryKind::SafeBoundary => Self::SafeBoundary,
            QueryKind::FinalizedHead => Self::FinalizedHead,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ExactQueryBinding {
    pub(super) kind: QueryKind,
    pub(super) request_digest: [u8; WORD_BYTES],
    pub(super) path_digest: [u8; WORD_BYTES],
    pub(super) calldata_digest: [u8; WORD_BYTES],
    pub(super) response_class: ExpectedResponseClass,
}

#[derive(Clone, Copy)]
struct QueryOffset {
    kind: QueryKind,
    start: usize,
    len: usize,
    binding: ExactQueryBinding,
}

const EMPTY_OFFSET: QueryOffset = QueryOffset {
    kind: QueryKind::SafeNonce,
    start: 0,
    len: 0,
    binding: ExactQueryBinding {
        kind: QueryKind::SafeNonce,
        request_digest: [0; WORD_BYTES],
        path_digest: [0; WORD_BYTES],
        calldata_digest: [0; WORD_BYTES],
        response_class: ExpectedResponseClass::SafeNonce,
    },
};

pub struct ExactQuerySet {
    bytes: CappedBytes,
    offsets: Box<[QueryOffset]>,
    len: usize,
}

impl ExactQuerySet {
    pub fn after_original_response_loss(
        profile: &ValidatedRedemptionProfile,
        attempt: &OriginalMayHaveStartedRequest,
        relayer: Option<&RelayerObservation>,
    ) -> Result<Self, QueryError> {
        Self::for_response_loss(profile, attempt.prepared(), relayer)
    }

    pub fn after_fence_response_loss(
        profile: &ValidatedRedemptionProfile,
        attempt: &FenceMayHaveStartedRequest,
        relayer: Option<&RelayerObservation>,
    ) -> Result<Self, QueryError> {
        Self::for_response_loss(profile, attempt.prepared(), relayer)
    }

    fn for_response_loss(
        profile: &ValidatedRedemptionProfile,
        prepared: &PreparedRequestPair,
        relayer: Option<&RelayerObservation>,
    ) -> Result<Self, QueryError> {
        if !prepared.matches_profile(profile) {
            return Err(QueryError::IntegrityFailure);
        }
        let mut result = Self::empty(profile);
        if let Some(observation) = relayer {
            let transaction_id = observation.transaction_id();
            result.append(
                QueryKind::RelayerTransaction,
                profile.transaction_path().as_bytes(),
                &[],
                |bytes| {
                    bytes.extend(br#"{"kind":"relayer_transaction","path":"#)?;
                    bytes.append_json_string(profile.transaction_path().as_bytes())?;
                    bytes.extend(br#","transaction_id":"#)?;
                    bytes.append_json_string(transaction_id)?;
                    bytes.push(b'}')
                },
            )?;
        }
        let safe = profile.safe_address();
        result.append(
            QueryKind::SafeNonce,
            profile.chain_path().as_bytes(),
            &profile.nonce_selector(),
            |bytes| {
                bytes.extend(br#"{"kind":"safe_nonce","safe":"0x"#)?;
                bytes.append_hex(&safe)?;
                bytes.extend(br#","calldata":"0x"#)?;
                bytes.append_hex(&profile.nonce_selector())?;
                bytes.extend(br#""}"#)
            },
        )?;
        for (kind, request_kind) in [
            (
                QueryKind::OriginalFinalizedReceiptLogs,
                RequestKind::Original,
            ),
            (QueryKind::FenceFinalizedReceiptLogs, RequestKind::Fence),
        ] {
            let hash = prepared
                .request(request_kind)
                .identity()
                .safe_transaction_hash();
            result.append(
                kind,
                profile.chain_path().as_bytes(),
                prepared.request(request_kind).calldata(),
                |bytes| {
                    bytes.extend(br#"{"kind":"finalized_receipt_logs","safe":"0x"#)?;
                    bytes.append_hex(&safe)?;
                    bytes.extend(br#","safe_transaction_hash":"0x"#)?;
                    bytes.append_hex(&hash)?;
                    bytes.extend(br#"","required_confirmations":"#)?;
                    append_decimal(bytes, profile.finality_confirmations())?;
                    bytes.extend(br#","max_logs":"#)?;
                    append_decimal(bytes, profile.max_receipt_logs() as u64)?;
                    bytes.push(b'}')
                },
            )?;
        }
        let request = prepared.request(RequestKind::Original);
        let (collateral, output_asset) = profile.post_state_assets();
        result.append(
            QueryKind::RawPostState,
            profile.chain_path().as_bytes(),
            request.calldata(),
            |bytes| {
                bytes.extend(br#"{"kind":"raw_post_state","target":"0x"#)?;
                bytes.append_hex(&request.identity().target())?;
                bytes.extend(br#","condition_id":"0x"#)?;
                bytes.append_hex(&prepared.condition_id())?;
                bytes.extend(br#","collateral":"0x"#)?;
                bytes.append_hex(&collateral)?;
                bytes.extend(br#","output_asset":"0x"#)?;
                bytes.append_hex(&output_asset)?;
                bytes.extend(br#","account":"0x"#)?;
                bytes.append_hex(&safe)?;
                bytes.extend(br#","pre_claim_balances":["0x"#)?;
                bytes.append_hex(&prepared.pre_claim_balances()[0])?;
                bytes.extend(br#"","0x"#)?;
                bytes.append_hex(&prepared.pre_claim_balances()[1])?;
                bytes.extend(br#""],"pre_collateral_balance":"0x"#)?;
                bytes.append_hex(&prepared.pre_collateral_balance())?;
                bytes.extend(br#"","expected_redeemed_collateral_balance":"0x"#)?;
                bytes.append_hex(&prepared.expected_redeemed_collateral_balance())?;
                bytes.extend(br#""}"#)
            },
        )?;
        let (factory, implementation, fallback_handler, guard) = profile.safe_boundary();
        result.append(
            QueryKind::SafeBoundary,
            profile.chain_path().as_bytes(),
            &[],
            |bytes| {
                bytes.extend(br#"{"kind":"safe_boundary","safe":"0x"#)?;
                bytes.append_hex(&safe)?;
                bytes.extend(br#","factory":"0x"#)?;
                bytes.append_hex(&factory)?;
                bytes.extend(br#","implementation":"0x"#)?;
                bytes.append_hex(&implementation)?;
                bytes.extend(br#","fallback_handler":"0x"#)?;
                bytes.append_hex(&fallback_handler)?;
                bytes.extend(br#","guard":"0x"#)?;
                bytes.append_hex(&guard)?;
                bytes.extend(br#"","modules":[]}"#)
            },
        )?;
        result.append(
            QueryKind::FinalizedHead,
            profile.chain_path().as_bytes(),
            &[],
            |bytes| bytes.extend(br#"{"kind":"finalized_head"}"#),
        )?;
        Ok(result)
    }

    pub fn count(&self) -> usize {
        self.len
    }

    pub fn kind_count(&self, kind: QueryKind) -> usize {
        self.offsets[..self.len]
            .iter()
            .filter(|offset| offset.kind == kind)
            .count()
    }

    pub fn projection(&self, credentials: &ResolvedRedemptionCredentials) -> RedactedProjection {
        self.bytes.projection(
            ProjectionClass::QuerySet,
            self.len,
            credentials.redaction_hmac_key(),
            credentials.key_version(),
        )
    }

    #[cfg(test)]
    pub(super) fn query_bytes(&self, index: usize) -> Result<&[u8], QueryError> {
        let offset = self
            .offsets
            .get(index)
            .filter(|_| index < self.len)
            .ok_or(QueryError::Index)?;
        Ok(&self.bytes.as_slice()[offset.start..offset.start + offset.len])
    }

    pub(super) fn binding(&self, kind: QueryKind) -> Result<ExactQueryBinding, QueryError> {
        let mut matches = self.offsets[..self.len]
            .iter()
            .filter(|offset| offset.kind == kind);
        let binding = matches.next().ok_or(QueryError::Index)?.binding;
        if matches.next().is_some() {
            return Err(QueryError::BindingMismatch);
        }
        Ok(binding)
    }

    fn empty(profile: &ValidatedRedemptionProfile) -> Self {
        Self {
            bytes: CappedBytes::with_capacity(profile.max_query_bytes()),
            offsets: vec![EMPTY_OFFSET; profile.max_query_items()].into_boxed_slice(),
            len: 0,
        }
    }

    fn append(
        &mut self,
        kind: QueryKind,
        path: &[u8],
        calldata: &[u8],
        write: impl FnOnce(&mut CappedBytes) -> Result<(), super::bounded::CappedIoError>,
    ) -> Result<(), QueryError> {
        if self.len == self.offsets.len() {
            return Err(QueryError::Capacity);
        }
        let start = self.bytes.len();
        write(&mut self.bytes).map_err(|_| QueryError::Capacity)?;
        let len = self.bytes.len() - start;
        let request_digest = Sha256::digest(&self.bytes.as_slice()[start..start + len]).into();
        self.offsets[self.len] = QueryOffset {
            kind,
            start,
            len,
            binding: ExactQueryBinding {
                kind,
                request_digest,
                path_digest: Sha256::digest(path).into(),
                calldata_digest: Sha256::digest(calldata).into(),
                response_class: ExpectedResponseClass::for_kind(kind),
            },
        };
        self.len += 1;
        Ok(())
    }
}

fn append_decimal(
    bytes: &mut CappedBytes,
    mut value: u64,
) -> Result<(), super::bounded::CappedIoError> {
    let mut reverse = [0u8; 20];
    if value == 0 {
        return bytes.push(b'0');
    }
    let mut len = 0;
    while value != 0 {
        reverse[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    for index in (0..len).rev() {
        bytes.push(reverse[index])?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryError {
    Capacity,
    Index,
    BindingMismatch,
    IntegrityFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostStateRelation {
    Redeemed,
    Unchanged,
    Drifted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonceRelation {
    Current,
    Successor,
    NoSuccessorAtMaximum,
    Unrelated,
}

pub fn classify_nonce_successor(prepared: SafeNonce, observed: SafeNonce) -> NonceRelation {
    if observed == prepared {
        return NonceRelation::Current;
    }
    match prepared.successor() {
        Some(successor) if observed == successor => NonceRelation::Successor,
        None => NonceRelation::NoSuccessorAtMaximum,
        Some(_) => NonceRelation::Unrelated,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedemptionResolution {
    Unresolved,
    RedemptionFinalized,
    PermanentlyFencedNoEffect,
    IntegrityFailure,
}

pub struct SourceBoundVerifiedOutcome {
    resolution: RedemptionResolution,
    profile_digest: [u8; WORD_BYTES],
    config_digest: [u8; WORD_BYTES],
    key_version: u32,
    chain_id: u64,
    relayer_source_identity: [u8; WORD_BYTES],
    chain_source_identity: [u8; WORD_BYTES],
    action_digest: [u8; WORD_BYTES],
    condition_id: [u8; WORD_BYTES],
    pre_claim_balances: [[u8; WORD_BYTES]; 2],
    pre_collateral_balance: [u8; WORD_BYTES],
    expected_redeemed_collateral_balance: [u8; WORD_BYTES],
    safe_nonce: SafeNonce,
    original_body_hash: [u8; WORD_BYTES],
    fence_body_hash: [u8; WORD_BYTES],
    finalized_block_number: [u8; WORD_BYTES],
    finalized_block_hash: [u8; WORD_BYTES],
    fence_authorized: bool,
}

pub(super) struct VerifiedOutcomeBinding {
    pub(super) finalized_block_number: [u8; WORD_BYTES],
    pub(super) finalized_block_hash: [u8; WORD_BYTES],
    pub(super) fence_authorized: bool,
}

impl SourceBoundVerifiedOutcome {
    pub(super) fn from_raw_verifier(
        resolution: RedemptionResolution,
        profile: &ValidatedRedemptionProfile,
        prepared: &PreparedRequestPair,
        binding: VerifiedOutcomeBinding,
    ) -> Self {
        let [original_body_hash, fence_body_hash] = prepared.body_hashes();
        let (
            profile_digest,
            config_digest,
            relayer_source_identity,
            chain_source_identity,
            key_version,
        ) = prepared.context_identity();
        Self {
            resolution,
            profile_digest,
            config_digest,
            key_version,
            chain_id: profile.chain_id(),
            relayer_source_identity,
            chain_source_identity,
            action_digest: prepared.action_digest(),
            condition_id: prepared.condition_id(),
            pre_claim_balances: prepared.pre_claim_balances(),
            pre_collateral_balance: prepared.pre_collateral_balance(),
            expected_redeemed_collateral_balance: prepared.expected_redeemed_collateral_balance(),
            safe_nonce: prepared.safe_nonce(),
            original_body_hash,
            fence_body_hash,
            finalized_block_number: binding.finalized_block_number,
            finalized_block_hash: binding.finalized_block_hash,
            fence_authorized: binding.fence_authorized,
        }
    }

    pub fn consume_after_original(
        self,
        responses: &ExactQueryResponses,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        attempt: &OriginalMayHaveStartedRequest,
    ) -> Result<RedemptionResolution, QueryError> {
        self.consume(responses, profile, credentials, attempt.prepared(), false)
    }

    pub fn consume_after_fence(
        self,
        responses: &ExactQueryResponses,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        attempt: &FenceMayHaveStartedRequest,
    ) -> Result<RedemptionResolution, QueryError> {
        self.consume(responses, profile, credentials, attempt.prepared(), true)
    }

    fn consume(
        self,
        responses: &ExactQueryResponses,
        profile: &ValidatedRedemptionProfile,
        credentials: &ResolvedRedemptionCredentials,
        prepared: &PreparedRequestPair,
        fence_authorized: bool,
    ) -> Result<RedemptionResolution, QueryError> {
        let [original_body_hash, fence_body_hash] = prepared.body_hashes();
        if !prepared.matches_context(profile, credentials)
            || self.profile_digest != profile.profile_digest()
            || self.config_digest != profile.config_digest()
            || self.key_version != credentials.key_version()
            || self.chain_id != profile.chain_id()
            || self.relayer_source_identity != profile.relayer_source_identity()
            || self.chain_source_identity != profile.chain_source_identity()
            || !responses.matches_context_binding(profile, credentials)
        {
            return Err(QueryError::IntegrityFailure);
        }
        if self.action_digest != prepared.action_digest()
            || self.condition_id != prepared.condition_id()
            || self.pre_claim_balances != prepared.pre_claim_balances()
            || self.pre_collateral_balance != prepared.pre_collateral_balance()
            || self.expected_redeemed_collateral_balance
                != prepared.expected_redeemed_collateral_balance()
            || self.safe_nonce != prepared.safe_nonce()
            || self.original_body_hash != original_body_hash
            || self.fence_body_hash != fence_body_hash
            || self.finalized_block_number == [0; WORD_BYTES]
            || self.finalized_block_hash == [0; WORD_BYTES]
            || self.fence_authorized != fence_authorized
            || !responses.matches_terminal_binding(
                profile,
                credentials,
                prepared,
                self.finalized_block_number,
                self.finalized_block_hash,
            )
        {
            return Err(QueryError::BindingMismatch);
        }
        Ok(self.resolution)
    }
}
