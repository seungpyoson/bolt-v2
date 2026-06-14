//! Strategy-archetype binding for the inert `binary_oracle_maker` (Slice 1, #488).
//!
//! This module lives under `src/strategies/` (the NON-scanned strategy layer),
//! so it may freely name both the maker strategy layer
//! (`crate::strategies::binary_oracle_maker::*`, `production_strategy_registry`)
//! and the shared bolt-v3 registration/live-node types. It mirrors the taker
//! archetype binding (`crate::bolt_v3_archetypes::binary_oracle_edge_taker`)
//! *structurally* — it owns:
//!
//! 1. `validate_strategy` — the maker's bolt-v3 startup-validation policy. The
//!    inert maker has no `[parameters]` rows yet, so this is **envelope-only**:
//!    it accepts the maker archetype key and emits no parameter-row errors.
//! 2. `register_runtime_strategy` — resolves the configured fee provider and
//!    execution venue, builds the **minimal** `StrategyBuildContext` + raw config
//!    table the inert maker consumes, and registers the strategy through the
//!    shared `production_strategy_registry()`.
//! 3. `RUNTIME_BINDING` — the `StrategyRuntimeBinding` the production aggregator
//!    (`crate::strategy_bindings`) lists alongside the taker binding.

use rust_decimal::Decimal;
use toml::{Value, map::Map};

use nautilus_model::identifiers::StrategyId;

use crate::bolt_v3_config::{BoltV3StrategyConfig, LoadedStrategy};
use crate::bolt_v3_providers::resolve_fee_provider;
use crate::bolt_v3_strategy_registration::{
    BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
};
use crate::strategies::binary_oracle_maker::{BinaryOracleMakerBuilder, KEY};
use crate::strategies::production_strategy_registry;
use crate::strategies::registry::StrategyBuilder;

/// The maker runtime binding the production aggregator lists. `key` and
/// `strategy_kind` both resolve to the single archetype constant
/// `binary_oracle_maker`; `register` is this module's `register_runtime_strategy`.
pub const RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding {
    key: KEY,
    strategy_kind: BinaryOracleMakerBuilder::kind,
    register: register_runtime_strategy,
};

/// Bolt-v3 startup validation for the inert maker.
///
/// Slice 1 is registered-but-inert: there are no `[parameters]` rows to check
/// yet, so this validator only confirms the archetype key and returns no errors
/// for any otherwise-structurally-valid maker envelope. `context` and
/// `_default_max_notional` mirror the taker validator's signature so the function
/// is assignable to `ArchetypeValidationBinding::validate_strategy`; the
/// risk-cap parameter is unused until later slices add notional parameters.
pub fn validate_strategy(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    _default_max_notional: Option<&Decimal>,
) -> Vec<String> {
    if strategy.strategy_archetype.as_str() != KEY {
        return vec![format!(
            "{context}: strategy_archetype `{}` is not `{KEY}`",
            strategy.strategy_archetype.as_str()
        )];
    }
    Vec::new()
}

/// Register the inert maker on the live node.
///
/// Mirrors the taker's `register_runtime_strategy` structurally: resolve the fee
/// provider and execution venue from the loaded config, build a
/// `StrategyBuildContext`, then hand the minimal raw config table to the shared
/// `production_strategy_registry()`. The raw table carries only the
/// NautilusTrader envelope fields the inert maker config consumes
/// (`strategy_id`, `order_id_tag`, `oms_type`).
pub fn register_runtime_strategy(
    node: &mut nautilus_live::node::LiveNode,
    context: StrategyRegistrationContext<'_>,
) -> Result<StrategyId, BoltV3StrategyRegistrationError> {
    let raw =
        raw_maker_config(context.strategy).map_err(|message| binding_message(&context, message))?;
    let fee_provider = resolve_fee_provider(
        context.loaded,
        context.strategy.config.execution_client_id.as_str(),
        context.resolved,
    )
    .map_err(|error| binding_message(&context, error.to_string()))?;
    let execution_client_id = context.strategy.config.execution_client_id.as_str();
    let execution_venue = context
        .loaded
        .root
        .clients
        .get(execution_client_id)
        .map(|client| client.venue)
        .ok_or_else(|| {
            binding_message(
                &context,
                format!(
                    "execution_client_id `{execution_client_id}` is not present in loaded clients for execution-venue resolution"
                ),
            )
        })?;
    let build_context = crate::strategies::registry::StrategyBuildContext::new(
        fee_provider,
        context.decision_evidence.clone(),
        context.submit_admission.clone(),
        execution_venue,
    )
    .with_realized_volatility_runtime(context.realized_volatility_runtime.clone());
    let registry = production_strategy_registry()
        .map_err(|error| binding_message(&context, error.to_string()))?;
    registry
        .register_strategy(
            BinaryOracleMakerBuilder::kind(),
            &raw,
            &build_context,
            node.kernel().trader(),
        )
        .map_err(|error| binding_message(&context, error.to_string()))
}

/// Build the minimal raw config table the inert maker consumes. The NautilusTrader
/// strategy id is `<strategy_archetype>-<order_id_tag>` (validated as an NT
/// `StrategyId`), mirroring the taker's `nt_strategy_id`; `oms_type` is the
/// lowercased NT enum display, matching how the maker config deserializes it.
fn raw_maker_config(strategy: &LoadedStrategy) -> Result<Value, String> {
    if strategy.config.strategy_archetype.as_str() != KEY {
        return Err(format!(
            "strategy_archetype `{}` is not `{KEY}`",
            strategy.config.strategy_archetype.as_str()
        ));
    }
    let mut strategy_id = strategy.config.strategy_archetype.as_str().to_string();
    strategy_id.push('-');
    strategy_id.push_str(&strategy.config.order_id_tag);
    StrategyId::new_checked(&strategy_id)
        .map_err(|error| format!("maps to invalid NT StrategyId `{strategy_id}`: {error}"))?;

    let mut table = Map::new();
    table.insert("strategy_id".to_string(), Value::String(strategy_id));
    table.insert(
        "order_id_tag".to_string(),
        Value::String(strategy.config.order_id_tag.clone()),
    );
    table.insert(
        "oms_type".to_string(),
        Value::String(strategy.config.oms_type.to_string().to_ascii_lowercase()),
    );
    Ok(Value::Table(table))
}

/// Wrap a registration failure message in the shared
/// `BoltV3StrategyRegistrationError::Binding` variant, mirroring the taker's
/// `binding_message`.
fn binding_message(
    context: &StrategyRegistrationContext<'_>,
    message: String,
) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::Binding {
        strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
        strategy_archetype: context
            .strategy
            .config
            .strategy_archetype
            .as_str()
            .to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_binding_key_is_archetype_key() {
        assert_eq!(RUNTIME_BINDING.key, "binary_oracle_maker");
        assert_eq!(RUNTIME_BINDING.key, KEY);
    }

    #[test]
    fn runtime_binding_strategy_kind_matches_key() {
        assert_eq!((RUNTIME_BINDING.strategy_kind)(), KEY);
    }
}
