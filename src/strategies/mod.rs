use anyhow::Result;

pub mod binary_oracle_edge_taker;
pub mod maker_reservation;
pub mod registry;

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
