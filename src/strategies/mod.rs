use anyhow::Result;

pub mod exec_tester;
pub mod registry;

use registry::StrategyRegistry;

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    let mut registry = StrategyRegistry::new();
    registry.register::<exec_tester::ExecTesterBuilder>()?;
    Ok(registry)
}
