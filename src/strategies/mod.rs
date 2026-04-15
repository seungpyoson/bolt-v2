use anyhow::Result;

pub mod registry;

use registry::StrategyRegistry;

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    Ok(StrategyRegistry::new())
}
