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

        #[cfg(test)]
        impl $crate::strategies::FillVoidPolicyGuard for $strategy {}
    };
}

pub(crate) use nautilus_strategy_with_fill_void_guard;

#[cfg(test)]
pub(crate) trait FillVoidPolicyGuard {}

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

    fn assert_fill_void_policy<T: FillVoidPolicyGuard>() {
        let _ = std::marker::PhantomData::<T>;
    }

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
        assert_fill_void_policy::<binary_oracle_edge_taker::BinaryOracleEdgeTaker>();
        assert_fill_void_policy::<binary_oracle_maker::BinaryOracleMaker>();
        assert_fill_void_policy::<complete_set_arbitrage::CompleteSetArbitrage>();

        let guarded_kinds = std::collections::BTreeSet::from([
            binary_oracle_edge_taker::KEY,
            binary_oracle_maker::KEY,
            complete_set_arbitrage::KEY,
        ]);
        let registered_kinds = production_strategy_registry()
            .expect("production strategy registry should build")
            .kinds()
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            guarded_kinds, registered_kinds,
            "fill-void guard must cover exactly the production strategy registry"
        );
    }
}
