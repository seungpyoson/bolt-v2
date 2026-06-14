use anyhow::Result;

pub mod binary_oracle_edge_taker;
pub mod complete_set_arbitrage;
pub mod binary_oracle_maker;
pub mod registry;

use registry::StrategyRegistry;

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    let mut registry = StrategyRegistry::new();
    registry.register::<binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder>()?;
    registry.register::<complete_set_arbitrage::CompleteSetArbitrageBuilder>()?;
    registry.register::<binary_oracle_maker::BinaryOracleMakerBuilder>()?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_registry_contains_expected_archetypes() {
        let registry =
            production_strategy_registry().expect("production strategy registry should build");
        let kinds = registry.kinds();
        assert!(
            kinds.contains(&binary_oracle_edge_taker::KEY),
            "taker archetype must stay registered, got: {kinds:?}"
        );
        assert!(
            kinds.contains(&complete_set_arbitrage::KEY),
            "complete-set arbitrage archetype must stay registered, got: {kinds:?}"
        );
        assert!(
            kinds.contains(&binary_oracle_maker::KEY),
            "maker archetype must be registered, got: {kinds:?}"
        );
    }
}
