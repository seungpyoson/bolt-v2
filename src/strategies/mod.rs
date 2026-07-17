use anyhow::Result;

pub mod binary_oracle_edge_taker;
pub mod binary_oracle_maker;
pub mod complete_set_arbitrage;
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

    #[test]
    fn production_strategies_override_the_fill_voided_no_op() {
        for (kind, source) in [
            (
                binary_oracle_edge_taker::KEY,
                include_str!("binary_oracle_edge_taker/mod.rs"),
            ),
            (
                binary_oracle_maker::KEY,
                include_str!("binary_oracle_maker/mod.rs"),
            ),
            (
                complete_set_arbitrage::KEY,
                include_str!("complete_set_arbitrage/mod.rs"),
            ),
        ] {
            assert!(
                source.contains("fn on_order_fill_voided"),
                "registered strategy {kind} must override NT's fill-voided no-op"
            );
            assert!(
                source.contains("fail_closed_on_order_fill_voided(event)"),
                "registered strategy {kind} must use the shared fill-voided boundary"
            );
        }
    }
}
