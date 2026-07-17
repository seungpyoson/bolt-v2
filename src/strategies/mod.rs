use anyhow::Result;

macro_rules! nautilus_strategy_with_fill_void_guard {
    ($strategy:ty, { $($hooks:item)* }) => {
        nautilus_trading::nautilus_strategy!($strategy, {
            $($hooks)*

            fn on_order_fill_voided(
                &mut self,
                event: &nautilus_model::events::OrderFillVoided,
            ) {
                $crate::bolt_v3_order_execution::fail_closed_on_order_fill_voided(event);
            }
        });

        impl $crate::strategies::FillVoidPolicyGuard for $strategy {}
    };
}

pub(crate) use nautilus_strategy_with_fill_void_guard;

pub(crate) trait FillVoidPolicyGuard {}

pub mod binary_oracle_edge_taker;
pub mod binary_oracle_maker;
pub mod complete_set_arbitrage;
pub mod registry;

use registry::StrategyRegistry;

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    let mut registry = StrategyRegistry::new();
    registry.register_guarded::<binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder>()?;
    registry.register_guarded::<complete_set_arbitrage::CompleteSetArbitrageBuilder>()?;
    registry.register_guarded::<binary_oracle_maker::BinaryOracleMakerBuilder>()?;
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
