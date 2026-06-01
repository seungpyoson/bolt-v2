use anyhow::Result;

pub mod binary_oracle_edge_taker;
pub mod maker_config;
pub mod maker_event_fence;
pub mod maker_governor;
pub mod maker_inventory;
pub mod maker_maintenance;
pub mod maker_microprice;
pub mod maker_model;
pub mod maker_offsets;
pub mod maker_quote;
pub mod maker_resync;
pub mod maker_stale_quote;
pub mod quote_lifecycle;
pub mod registry;
pub mod requote_budget;

use crate::bolt_v3_canary_proof_policy::{CanaryProofCandidate, CanaryProofSourcePacket};
use registry::StrategyRegistry;

pub trait CanaryProofCandidateProvider {
    fn canary_proof_candidates(
        &self,
        source_packet: &CanaryProofSourcePacket,
    ) -> Result<Vec<CanaryProofCandidate>>;
}

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    let mut registry = StrategyRegistry::new();
    registry.register::<binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder>()?;
    Ok(registry)
}
