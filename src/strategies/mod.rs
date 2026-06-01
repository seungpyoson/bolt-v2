use anyhow::Result;

pub mod binary_oracle_edge_taker;
pub mod quote_lifecycle;
pub mod registry;
pub mod requote_budget;

use registry::StrategyRegistry;

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    let mut registry = StrategyRegistry::new();
    registry.register::<binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder>()?;
    Ok(registry)
}
