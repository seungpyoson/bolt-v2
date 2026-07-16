//! Production strategy-archetype binding lists, assembled in a NON-scanned
//! crate-root module so it may name both the shared-layer (`crate::bolt_v3_*`)
//! and strategy-layer (`crate::strategies::*`) bindings without violating the
//! dependency-direction fence (forbidden root is `strategies`, not this module)
//! and without growing `FINDING_ALLOWANCES`.
//!
//! These lists were previously the `RUNTIME_BINDINGS` / `VALIDATION_BINDINGS`
//! production constants inside the scanned `crate::bolt_v3_archetypes` module.
//! They were hoisted here so a strategy-layer archetype binding (the maker's
//! `crate::strategies::binary_oracle_maker::archetype`) can be listed without a
//! scanned `src/bolt_v3_*` file referencing `crate::strategies`. The generic
//! dispatch (`validate_strategy_archetype_with_bindings`,
//! `register_bolt_v3_strategies_on_node_with_bindings`) still lives in the shared
//! layer and takes these lists as parameters — there is no second registry.
use crate::bolt_v3_archetypes::ArchetypeValidationBinding;
use crate::bolt_v3_strategy_registration::StrategyRuntimeBinding;
use crate::strategies::{binary_oracle_edge_taker, binary_oracle_maker, complete_set_arbitrage};

const RUNTIME_BINDINGS: &[StrategyRuntimeBinding] = &[
    binary_oracle_edge_taker::archetype::RUNTIME_BINDING,
    complete_set_arbitrage::archetype::RUNTIME_BINDING,
    binary_oracle_maker::archetype::RUNTIME_BINDING,
];

const VALIDATION_BINDINGS: &[ArchetypeValidationBinding] = &[
    ArchetypeValidationBinding {
        key: binary_oracle_edge_taker::archetype::KEY,
        validate_strategy: binary_oracle_edge_taker::archetype::validate_strategy,
    },
    ArchetypeValidationBinding {
        key: complete_set_arbitrage::archetype::KEY,
        validate_strategy: complete_set_arbitrage::archetype::validate_strategy,
    },
    ArchetypeValidationBinding {
        key: binary_oracle_maker::KEY,
        validate_strategy: binary_oracle_maker::archetype::validate_strategy,
    },
];

/// The production runtime-binding list: every archetype's
/// `StrategyRuntimeBinding`, used by the live node to register strategies.
pub fn production_runtime_bindings() -> &'static [StrategyRuntimeBinding] {
    RUNTIME_BINDINGS
}

/// The production validation-binding list: every archetype's
/// `ArchetypeValidationBinding`, used by bolt-v3 startup validation.
pub fn production_validation_bindings() -> &'static [ArchetypeValidationBinding] {
    VALIDATION_BINDINGS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_bindings_carry_expected_archetypes() {
        let keys: Vec<&str> = production_runtime_bindings()
            .iter()
            .map(|binding| binding.key)
            .collect();
        assert!(keys.contains(&"binary_oracle_edge_taker"), "{keys:?}");
        assert!(keys.contains(&"complete_set_arbitrage"), "{keys:?}");
        assert!(keys.contains(&"binary_oracle_maker"), "{keys:?}");
    }

    #[test]
    fn runtime_bindings_carry_expected_capability_matrix() {
        let capabilities = production_runtime_bindings()
            .iter()
            .map(|binding| (binding.key, binding.capabilities))
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(
            capabilities["binary_oracle_edge_taker"],
            crate::bolt_v3_strategy_registration::StrategyRuntimeCapabilities {
                realized_volatility: true,
                settlement: true,
            }
        );
        for key in ["binary_oracle_maker", "complete_set_arbitrage"] {
            assert_eq!(
                capabilities[key],
                crate::bolt_v3_strategy_registration::StrategyRuntimeCapabilities {
                    realized_volatility: true,
                    settlement: false,
                },
                "{key} capability declaration drifted"
            );
        }
    }

    #[test]
    fn validation_bindings_carry_expected_archetypes() {
        let keys: Vec<&str> = production_validation_bindings()
            .iter()
            .map(|binding| binding.key)
            .collect();
        assert!(keys.contains(&"binary_oracle_edge_taker"), "{keys:?}");
        assert!(keys.contains(&"complete_set_arbitrage"), "{keys:?}");
        assert!(keys.contains(&"binary_oracle_maker"), "{keys:?}");
    }
}
