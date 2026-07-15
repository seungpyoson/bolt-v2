//! Pure, mechanically disabled Polymarket relayer/Safe redemption primitive.
//!
//! This module owns encoding, exact-id query descriptions, bounded wire decoding,
//! and deterministic winner classification only. It deliberately owns no transport,
//! scheduler, durable state, or invocation path. A later Capsule integration must
//! supply authority and retain the exact opaque request bytes.

mod config;
mod query;
mod request;
mod wire;

pub use config::{
    ProviderManifest, RedemptionConfig, RedemptionConfigError, ResolvedRedemptionCredentials,
    ValidatedRedemptionProfile, resolve_credentials, validate_profile,
};
pub use query::{
    ExactQuery, ExactQuerySet, ExecutionProof, PostStateRelation, RedemptionResolution,
    ResolutionObservation, resolve_competing_nonce,
};
pub use request::{
    ActionIdentity, MarketMode, PreparedRequestPair, RedactedRequestDescriptor,
    RedemptionBuildInput, RedemptionRequestError, SensitiveRequestBytes, build_request_pair,
    require_exact_retry, revalidate_pre_send,
};
pub use wire::{
    BoundedWireResponse, RelayerState, RelayerTransaction, SubmitResponse, WireDiagnostic,
    WireFailureClass, WireParseError,
};

pub const POLYMARKET_RELAYER_ADAPTER_ID: &str = stringify!(PolymarketSafeRelayer);
pub const POLYGON_REDEMPTION_RPC_ADAPTER_ID: &str = stringify!(PolygonRedemptionRpc);

/// The primitive cannot become reachable merely by constructing a valid profile.
/// Hosted competing-same-nonce conformance is intentionally unproven in #1384.
pub const MECHANICALLY_ENABLED: bool = false;

pub fn activation_capable(profile: &ValidatedRedemptionProfile) -> bool {
    MECHANICALLY_ENABLED && profile.config.relayer.competing_same_nonce_conformance
}
