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
        let cases = [
            (
                binary_oracle_edge_taker::KEY,
                "nautilus_strategy!(BinaryOracleEdgeTaker, {",
                include_str!("binary_oracle_edge_taker/mod.rs"),
            ),
            (
                binary_oracle_maker::KEY,
                "nautilus_strategy!(BinaryOracleMaker, {",
                include_str!("binary_oracle_maker/mod.rs"),
            ),
            (
                complete_set_arbitrage::KEY,
                "nautilus_strategy!(CompleteSetArbitrage, {",
                include_str!("complete_set_arbitrage/mod.rs"),
            ),
        ];
        let guarded_kinds = cases
            .iter()
            .map(|(kind, _, _)| *kind)
            .collect::<std::collections::BTreeSet<_>>();
        let registered_kinds = production_strategy_registry()
            .expect("production strategy registry should build")
            .kinds()
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            guarded_kinds, registered_kinds,
            "fill-void guard must cover exactly the production strategy registry"
        );

        for (kind, macro_marker, source) in cases {
            let (_, hook_source) = source
                .split_once(macro_marker)
                .expect("registered strategy must use its declared hook macro");
            let (hooks, _) = hook_source
                .split_once("});")
                .expect("registered strategy hook macro must terminate");
            assert!(
                hooks.contains("fn on_order_fill_voided"),
                "registered strategy {kind} must override NT's fill-voided no-op"
            );
            assert!(
                hooks.contains("fail_closed_on_order_fill_voided(event)"),
                "registered strategy {kind} must use the shared fill-voided boundary"
            );
        }
    }
}
