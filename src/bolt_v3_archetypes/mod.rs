//! Strategy-archetype binding root for bolt-v3 startup validation.
//!
//! Core startup validation in `crate::bolt_v3_validate` is structural
//! and family-/archetype-agnostic. Anything specific to a concrete
//! strategy archetype key — required reference-data roles,
//! the archetype's `[parameters]` row shape, archetype-shaped
//! parameter rules (entry/exit order combinations, decimal-syntax
//! checks, root risk-cap comparison), and archetype-specific error-
//! message policy — lives in a per-archetype binding module under
//! this root. This module is the family-agnostic dispatch layer: it
//! owns the generic dispatch fn
//! (`validate_strategy_archetype_with_bindings`) that, given an
//! injected binding list, calls into the matching archetype binding so
//! core validation does not name any concrete archetype variant,
//! deserialize the archetype's parameter row, or carry
//! archetype-specific error wording. Core validation parses the root
//! risk cap once and passes it in here as
//! `default_max_notional_decimal`.
//!
//! The production binding *lists* themselves live in the non-scanned
//! crate-root module `crate::strategy_bindings`
//! (`production_validation_bindings` / `production_runtime_bindings`),
//! so a strategy-layer archetype binding (the maker's
//! `crate::strategies::binary_oracle_maker::archetype`) can be listed
//! without a scanned `src/bolt_v3_*` file referencing
//! `crate::strategies`. When a new archetype is introduced it adds its
//! own per-archetype binding module and one entry in those lists; this
//! dispatch fn does not change.

pub mod binary_oracle_edge_taker;
pub mod complete_set_arbitrage;

use std::collections::BTreeSet;

use rust_decimal::Decimal;

use crate::bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GateRole {
    Resolution,
    DecisionReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GateValueKind {
    Price,
    Index,
    Outcome,
    Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeGateRequirement {
    pub role: GateRole,
    pub required: bool,
    pub accepted_value_kinds: BTreeSet<GateValueKind>,
    pub allow_no_resolution: bool,
}

pub struct ArchetypeValidationBinding {
    pub key: &'static str,
    pub validate_strategy:
        fn(&str, &BoltV3RootConfig, &BoltV3StrategyConfig, Option<&Decimal>) -> Vec<String>,
}

pub fn validate_strategy_archetype_with_bindings(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
    default_max_notional_decimal: Option<&Decimal>,
    bindings: &[ArchetypeValidationBinding],
) -> Vec<String> {
    match bindings
        .iter()
        .find(|binding| binding.key == strategy.strategy_archetype.as_str())
    {
        Some(binding) => {
            (binding.validate_strategy)(context, root, strategy, default_max_notional_decimal)
        }
        None => vec![format!(
            "{context}: strategy_archetype `{}` is not supported by this build",
            strategy.strategy_archetype.as_str()
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_validate_strategy(
        _context: &str,
        _root: &BoltV3RootConfig,
        _strategy: &BoltV3StrategyConfig,
        _default_max_notional_decimal: Option<&Decimal>,
    ) -> Vec<String> {
        Vec::new()
    }

    const FAKE_ARCHETYPE_BINDINGS: &[ArchetypeValidationBinding] = &[ArchetypeValidationBinding {
        key: "fixture_archetype",
        validate_strategy: fake_validate_strategy,
    }];

    #[test]
    fn validation_can_use_injected_archetype_binding_without_editing_production_registry() {
        let strategy: BoltV3StrategyConfig = toml::from_str(
            r#"
schema_version = 1
strategy_instance_id = "fixture-strategy"
strategy_archetype = "fixture_archetype"
order_id_tag = "FIXTURE"
oms_type = "netting"
use_uuid_client_order_ids = true
use_hyphens_in_client_order_ids = false
external_order_claims = []
manage_contingent_orders = false
manage_gtd_expiry = false
manage_stop = false
market_exit_interval_ms = 100
market_exit_max_attempts = 100
log_events = true
log_commands = true
log_rejected_due_post_only_as_warning = true
execution_client_id = "fixture-venue"

[signal_data]

[target]
configured_target_id = "fixture-target"
rotating_market_family = "fixture-family"

[parameters]
"#,
        )
        .expect("fixture strategy parses");
        let root: BoltV3RootConfig =
            toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml"))
                .expect("fixture root parses");

        let production_errors = validate_strategy_archetype_with_bindings(
            "strategy `fixture`",
            &root,
            &strategy,
            None,
            crate::strategy_bindings::production_validation_bindings(),
        );
        assert!(
            production_errors
                .iter()
                .any(|message| message.contains("not supported by this build")),
            "production registry should not know the test archetype: {production_errors:?}"
        );

        let injected_errors = validate_strategy_archetype_with_bindings(
            "strategy `fixture`",
            &root,
            &strategy,
            None,
            FAKE_ARCHETYPE_BINDINGS,
        );
        assert!(
            injected_errors.is_empty(),
            "injected archetype binding should own strategy dispatch: {injected_errors:?}"
        );
    }
}
