//! Boundary evidence registry for deploy/readiness feeder surfaces.

use super::{chainlink_reference, polymarket, polyresearch};

pub const AWS_SSM_SECRET_SOURCE_ADAPTER_ID: &str = stringify!(AwsSsmSecretSource);
pub const IMDS_METADATA_ADAPTER_ID: &str = stringify!(Imdsv2HostFactsSource);
pub const BINANCE_SPOT_SBE_ADAPTER_ID: &str = stringify!(BinanceSpotDataClient);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryEvidenceClass {
    WebSocketFrame,
    ImdsMetadata,
    AwsSdkResponse,
    HttpResponseBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryFeeder {
    ReferenceCurrentPriceHealth,
    ReferenceLiveProbe,
    RealizedVolatilityObservation,
    StrategySignalObservation,
    DeployTargetHostFacts,
    SecretResolution,
    PolymarketProviderCollateralAllowanceRuntime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundaryRegistryEntry {
    pub adapter_id: &'static str,
    pub class: BoundaryEvidenceClass,
    pub feeder: BoundaryFeeder,
}

pub const BOUNDARY_REGISTRY: &[BoundaryRegistryEntry] = &[
    BoundaryRegistryEntry {
        adapter_id: chainlink_reference::KEY,
        class: BoundaryEvidenceClass::WebSocketFrame,
        feeder: BoundaryFeeder::ReferenceCurrentPriceHealth,
    },
    BoundaryRegistryEntry {
        adapter_id: polyresearch::KEY,
        class: BoundaryEvidenceClass::WebSocketFrame,
        feeder: BoundaryFeeder::ReferenceCurrentPriceHealth,
    },
    BoundaryRegistryEntry {
        adapter_id: chainlink_reference::KEY,
        class: BoundaryEvidenceClass::WebSocketFrame,
        feeder: BoundaryFeeder::ReferenceLiveProbe,
    },
    BoundaryRegistryEntry {
        adapter_id: polyresearch::KEY,
        class: BoundaryEvidenceClass::WebSocketFrame,
        feeder: BoundaryFeeder::ReferenceLiveProbe,
    },
    BoundaryRegistryEntry {
        adapter_id: BINANCE_SPOT_SBE_ADAPTER_ID,
        class: BoundaryEvidenceClass::WebSocketFrame,
        feeder: BoundaryFeeder::RealizedVolatilityObservation,
    },
    BoundaryRegistryEntry {
        adapter_id: BINANCE_SPOT_SBE_ADAPTER_ID,
        class: BoundaryEvidenceClass::WebSocketFrame,
        feeder: BoundaryFeeder::StrategySignalObservation,
    },
    BoundaryRegistryEntry {
        adapter_id: IMDS_METADATA_ADAPTER_ID,
        class: BoundaryEvidenceClass::ImdsMetadata,
        feeder: BoundaryFeeder::DeployTargetHostFacts,
    },
    BoundaryRegistryEntry {
        adapter_id: AWS_SSM_SECRET_SOURCE_ADAPTER_ID,
        class: BoundaryEvidenceClass::AwsSdkResponse,
        feeder: BoundaryFeeder::SecretResolution,
    },
    BoundaryRegistryEntry {
        adapter_id: polymarket::KEY,
        class: BoundaryEvidenceClass::HttpResponseBody,
        feeder: BoundaryFeeder::PolymarketProviderCollateralAllowanceRuntime,
    },
];
