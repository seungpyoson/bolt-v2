use anyhow::Result;

pub mod binary_oracle_edge_taker;
pub mod binary_oracle_maker;
pub mod registry;

use registry::StrategyRegistry;

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    let mut registry = StrategyRegistry::new();
    registry.register::<binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder>()?;
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
        assert_eq!(
            kinds,
            vec![binary_oracle_edge_taker::KEY, binary_oracle_maker::KEY]
        );
    }
}
