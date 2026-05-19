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
//! owns the static archetype binding list and calls into the matching
//! archetype binding so core validation does not name any concrete
//! archetype variant, deserialize the archetype's parameter row, or
//! carry archetype-specific error wording. Core validation parses the
//! root risk cap once and passes it in here as
//! `default_max_notional_decimal`.
//!
//! Today bolt-v3 has a single archetype binding. When a second
//! archetype is introduced, it adds its own per-archetype module and
//! one entry in this root's binding list; core validation does not
//! change.

pub mod binary_oracle_edge_taker;

use rust_decimal::Decimal;

use crate::{
    bolt_v3_config::BoltV3StrategyConfig, bolt_v3_strategy_registration::StrategyRuntimeBinding,
};

pub struct ArchetypeValidationBinding {
    pub key: &'static str,
    pub validate_strategy: fn(&str, &BoltV3StrategyConfig, Option<&Decimal>) -> Vec<String>,
}

const VALIDATION_BINDINGS: &[ArchetypeValidationBinding] = &[ArchetypeValidationBinding {
    key: binary_oracle_edge_taker::KEY,
    validate_strategy: binary_oracle_edge_taker::validate_strategy,
}];

const RUNTIME_BINDINGS: &[StrategyRuntimeBinding] = &[binary_oracle_edge_taker::RUNTIME_BINDING];

pub fn validation_bindings() -> &'static [ArchetypeValidationBinding] {
    VALIDATION_BINDINGS
}

pub fn runtime_bindings() -> &'static [StrategyRuntimeBinding] {
    RUNTIME_BINDINGS
}

pub fn validate_strategy_archetype(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    default_max_notional_decimal: Option<&Decimal>,
) -> Vec<String> {
    validate_strategy_archetype_with_bindings(
        context,
        strategy,
        default_max_notional_decimal,
        validation_bindings(),
    )
}

pub fn validate_strategy_archetype_with_bindings(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    default_max_notional_decimal: Option<&Decimal>,
    bindings: &[ArchetypeValidationBinding],
) -> Vec<String> {
    match bindings
        .iter()
        .find(|binding| binding.key == strategy.strategy_archetype.as_str())
    {
        Some(binding) => {
            (binding.validate_strategy)(context, strategy, default_max_notional_decimal)
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
market_exit_time_in_force = "gtc"
market_exit_reduce_only = true
log_events = true
log_commands = true
log_rejected_due_post_only_as_warning = true
venue = "fixture-venue"

[reference_data.reference]
venue = "fixture-reference"
instrument_id = "FIXTURE.REFERENCE"

[target]
configured_target_id = "fixture-target"
rotating_market_family = "fixture-family"

[parameters]
"#,
        )
        .expect("fixture strategy parses");

        let production_errors = validate_strategy_archetype("strategy `fixture`", &strategy, None);
        assert!(
            production_errors
                .iter()
                .any(|message| message.contains("not supported by this build")),
            "production registry should not know the test archetype: {production_errors:?}"
        );

        let injected_errors = validate_strategy_archetype_with_bindings(
            "strategy `fixture`",
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
