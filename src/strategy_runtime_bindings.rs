//! Production runtime binding aggregation for bolt-v3 strategies.
//!
//! Validation bindings live with archetypes. Runtime bindings live here so
//! shared bolt-v3 modules do not depend on the concrete strategy layer.

use crate::{
    bolt_v3_archetypes::binary_oracle_edge_taker,
    bolt_v3_strategy_registration::StrategyRuntimeBinding, strategies::complete_set_arbitrage,
};

const RUNTIME_BINDINGS: &[StrategyRuntimeBinding] = &[
    binary_oracle_edge_taker::RUNTIME_BINDING,
    complete_set_arbitrage::RUNTIME_BINDING,
];

pub fn runtime_bindings() -> &'static [StrategyRuntimeBinding] {
    RUNTIME_BINDINGS
}
