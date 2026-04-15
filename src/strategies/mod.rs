use anyhow::Result;

pub mod eth_chainlink_taker;
pub mod registry;

use registry::StrategyRegistry;

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    let mut registry = StrategyRegistry::new();
    registry.register::<eth_chainlink_taker::EthChainlinkTakerBuilder>()?;
    Ok(registry)
}
