//! Pure, mechanically disabled Polymarket relayer/Safe redemption primitive.
//!
//! This module owns encoding, exact-id query descriptions, bounded wire decoding,
//! and deterministic winner classification only. It deliberately owns no transport,
//! scheduler, durable state, or invocation path. A later Capsule integration must
//! supply authority and retain the exact opaque request bytes.

mod bounded;
mod capability;
mod config;
mod nonce;
mod query;
mod request;
mod wire;

#[cfg(test)]
mod tests;

pub use bounded::{ProjectionClass, RedactedProjection};
pub use capability::{
    ExactConditionSnapshotLease, FenceMayHaveStartedPermit, FreshPreSendValidation,
    OriginalMayHaveStartedPermit, SafeNonceBodyCapacityPermit,
};
pub use config::{
    CappedSsmCredentialSource, CredentialSink, RedemptionConfigError,
    ResolvedRedemptionCredentials, ValidatedRedemptionProfile, resolve_credentials,
    validate_profile,
};
pub use nonce::{NonceError, SafeNonce};
pub use query::{
    ExactQuerySet, NonceRelation, PostStateRelation, QueryError, QueryKind, RedemptionResolution,
    SourceBoundVerifiedOutcome, classify_nonce_successor,
};
pub use request::{
    AuthorizedRequest, FenceMayHaveStartedRequest, MarketMode, OriginalMayHaveStartedRequest,
    PreparedRequestPair, RedemptionBuildInput, RedemptionRequestError, RequestKind,
    build_request_pair, require_exact_retry,
};
pub use wire::{
    ExactActionBinding, ExactQueryResponses, FinalizedChainSourceResponse, RelayerObservation,
    RelayerSourceResponse, RelayerState, WireDiagnostic, WireFailureClass, WireParseError,
};

pub const POLYMARKET_RELAYER_ADAPTER_ID: &str = stringify!(PolymarketSafeRelayer);
pub const POLYGON_REDEMPTION_RPC_ADAPTER_ID: &str = stringify!(PolygonRedemptionRpc);

/// The primitive cannot become reachable merely by constructing a valid profile.
/// Hosted competing-same-nonce conformance is intentionally unproven in #1384.
pub const MECHANICALLY_ENABLED: bool = false;
