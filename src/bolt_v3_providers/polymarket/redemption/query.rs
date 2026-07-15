use super::bounded::{CappedBytes, ProjectionClass, RedactedProjection};
use super::config::{ResolvedRedemptionCredentials, ValidatedRedemptionProfile};
use super::nonce::SafeNonce;
use super::request::{
    FenceMayHaveStartedRequest, OriginalMayHaveStartedRequest, PreparedRequestPair, RequestKind,
};
use super::wire::RelayerObservation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    RelayerTransaction,
    SafeNonce,
    OriginalFinalizedReceiptLogs,
    FenceFinalizedReceiptLogs,
    FinalizedHead,
    RawPostState,
    SafeBoundary,
}

#[derive(Clone, Copy)]
struct QueryOffset {
    kind: QueryKind,
    start: usize,
    len: usize,
}

const EMPTY_OFFSET: QueryOffset = QueryOffset {
    kind: QueryKind::SafeNonce,
    start: 0,
    len: 0,
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
        let mut result = Self::empty(profile);
        if let Some(observation) = relayer {
            let transaction_id = observation.transaction_id();
            result.append(QueryKind::RelayerTransaction, |bytes| {
                bytes.extend(br#"{"kind":"relayer_transaction","path":"#)?;
                bytes.append_json_string(profile.transaction_path().as_bytes())?;
                bytes.extend(br#","transaction_id":"#)?;
                bytes.append_json_string(transaction_id)?;
                bytes.push(b'}')
            })?;
        }
        let safe = profile.safe_address();
        result.append(QueryKind::SafeNonce, |bytes| {
            bytes.extend(br#"{"kind":"safe_nonce","safe":"0x"#)?;
            bytes.append_hex(&safe)?;
            bytes.extend(br#","calldata":"0x"#)?;
            bytes.append_hex(&profile.nonce_selector())?;
            bytes.extend(br#""}"#)
        })?;
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
            result.append(kind, |bytes| {
                bytes.extend(br#"{"kind":"finalized_receipt_logs","safe":"0x"#)?;
                bytes.append_hex(&safe)?;
                bytes.extend(br#","safe_transaction_hash":"0x"#)?;
                bytes.append_hex(&hash)?;
                bytes.extend(br#"","required_confirmations":"#)?;
                append_decimal(bytes, profile.finality_confirmations())?;
                bytes.extend(br#","max_logs":"#)?;
                append_decimal(bytes, profile.max_receipt_logs() as u64)?;
                bytes.push(b'}')
            })?;
        }
        let request = prepared.request(RequestKind::Original);
        result.append(QueryKind::RawPostState, |bytes| {
            bytes.extend(br#"{"kind":"raw_post_state","target":"0x"#)?;
            bytes.append_hex(&request.identity().target())?;
            bytes.extend(br#","condition_id":"0x"#)?;
            bytes.append_hex(&prepared.condition_id())?;
            bytes.extend(br#","pre_balances":["0x"#)?;
            bytes.append_hex(&prepared.pre_balances()[0])?;
            bytes.extend(br#"","0x"#)?;
            bytes.append_hex(&prepared.pre_balances()[1])?;
            bytes.extend(br#""]}"#)
        })?;
        let (factory, implementation, fallback_handler, guard) = profile.safe_boundary();
        result.append(QueryKind::SafeBoundary, |bytes| {
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
        })?;
        result.append(QueryKind::FinalizedHead, |bytes| {
            bytes.extend(br#"{"kind":"finalized_head"}"#)
        })?;
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
        write: impl FnOnce(&mut CappedBytes) -> Result<(), super::bounded::CappedIoError>,
    ) -> Result<(), QueryError> {
        if self.len == self.offsets.len() {
            return Err(QueryError::Capacity);
        }
        let start = self.bytes.len();
        write(&mut self.bytes).map_err(|_| QueryError::Capacity)?;
        let len = self.bytes.len() - start;
        self.offsets[self.len] = QueryOffset { kind, start, len };
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
}

impl SourceBoundVerifiedOutcome {
    pub(super) fn from_raw_verifier(resolution: RedemptionResolution) -> Self {
        Self { resolution }
    }

    pub fn resolution(&self) -> RedemptionResolution {
        self.resolution
    }
}
