use std::collections::BTreeSet;

use crate::bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy};
use crate::bolt_v3_order_execution::BoltV3OrderExecutionMode;

use super::{
    TARGET_ALLOWED_PROVIDER_IDS_FIELD, TARGET_GATE_SUBSCRIPTIONS_FIELD,
    TARGET_MARKET_MAPPINGS_FIELD, TARGET_PROVIDER_ID_FIELD, TARGET_PROVIDER_PREFERENCE_FIELD,
    string_array_values, string_field,
};

pub(super) fn validate_complete_set_activation_is_shadow_only(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    if strategy.strategy_archetype.as_str()
        != crate::bolt_v3_complete_set_contract::COMPLETE_SET_ARBITRAGE_KEY
        || root.runtime.order_execution_mode == BoltV3OrderExecutionMode::Shadow
    {
        return Vec::new();
    }

    vec![format!(
        "{context}: complete_set_arbitrage runtime activation is registration-only until NautilusTrader event forwarding is wired; runtime.order_execution_mode must be shadow for this substrate slice"
    )]
}

pub(super) fn validate_shadow_order_execution_mode_forbids_managed_venue_actions(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    if root.runtime.order_execution_mode != BoltV3OrderExecutionMode::Shadow {
        return Vec::new();
    }

    let mut errors = Vec::new();
    for (field, enabled) in [
        (stringify!(manage_stop), strategy.manage_stop),
        (stringify!(manage_gtd_expiry), strategy.manage_gtd_expiry),
        (
            stringify!(manage_contingent_orders),
            strategy.manage_contingent_orders,
        ),
    ] {
        if enabled {
            errors.push(format!(
                "{context}: runtime.order_execution_mode=shadow requires {field}=false because it drives NautilusTrader-managed venue actions outside the shared order-execution policy"
            ));
        }
    }
    if !strategy.external_order_claims.is_empty() {
        errors.push(format!(
            "{context}: runtime.order_execution_mode=shadow requires external_order_claims=[] because claimed foreign orders are managed by NautilusTrader outside the shared order-execution policy"
        ));
    }
    errors
}

pub(super) fn validate_target_gate_provider_references(
    root: &BoltV3RootConfig,
    strategies: &[LoadedStrategy],
) -> Vec<String> {
    let mut errors = Vec::new();
    let known_provider_ids = match &root.gate_providers {
        Some(providers) => providers.keys().cloned().collect::<BTreeSet<_>>(),
        None => BTreeSet::new(),
    };

    for loaded in strategies {
        let strategy_context = format!("strategy `{}`", loaded.relative_path);
        let Some(target) = loaded.config.target.as_table() else {
            continue;
        };
        let Some(gate_subscriptions) = target
            .get(TARGET_GATE_SUBSCRIPTIONS_FIELD)
            .and_then(toml::Value::as_table)
        else {
            continue;
        };
        for (role, subscription_value) in gate_subscriptions {
            let Some(subscription) = subscription_value.as_table() else {
                continue;
            };
            let subscription_context =
                format!("{strategy_context}: target.{TARGET_GATE_SUBSCRIPTIONS_FIELD}.{role}");
            let allowed_provider_ids =
                string_array_values(subscription, TARGET_ALLOWED_PROVIDER_IDS_FIELD);
            for provider_id in &allowed_provider_ids {
                validate_known_target_gate_provider_id(
                    &mut errors,
                    &known_provider_ids,
                    &format!("{subscription_context}.{TARGET_ALLOWED_PROVIDER_IDS_FIELD}"),
                    provider_id,
                );
            }

            for provider_id in string_array_values(subscription, TARGET_PROVIDER_PREFERENCE_FIELD) {
                validate_known_target_gate_provider_id(
                    &mut errors,
                    &known_provider_ids,
                    &format!("{subscription_context}.{TARGET_PROVIDER_PREFERENCE_FIELD}"),
                    &provider_id,
                );
                if !allowed_provider_ids.is_empty() && !allowed_provider_ids.contains(&provider_id)
                {
                    errors.push(format!(
                        "{subscription_context}.{TARGET_PROVIDER_PREFERENCE_FIELD} provider_id `{provider_id}` must also be listed in {TARGET_ALLOWED_PROVIDER_IDS_FIELD}"
                    ));
                }
            }

            let Some(market_mappings) = subscription
                .get(TARGET_MARKET_MAPPINGS_FIELD)
                .and_then(toml::Value::as_array)
            else {
                continue;
            };
            for (index, mapping_value) in market_mappings.iter().enumerate() {
                let Some(mapping) = mapping_value.as_table() else {
                    continue;
                };
                let Some(provider_id) = string_field(mapping, TARGET_PROVIDER_ID_FIELD) else {
                    continue;
                };
                let mapping_context =
                    format!("{subscription_context}.{TARGET_MARKET_MAPPINGS_FIELD}[{index}]");
                validate_known_target_gate_provider_id(
                    &mut errors,
                    &known_provider_ids,
                    &format!("{mapping_context}.{TARGET_PROVIDER_ID_FIELD}"),
                    &provider_id,
                );
                if !allowed_provider_ids.is_empty() && !allowed_provider_ids.contains(&provider_id)
                {
                    errors.push(format!(
                        "{mapping_context}.{TARGET_PROVIDER_ID_FIELD} `{provider_id}` must also be listed in {TARGET_ALLOWED_PROVIDER_IDS_FIELD}"
                    ));
                }
            }
        }
    }

    errors
}

fn validate_known_target_gate_provider_id(
    errors: &mut Vec<String>,
    known_provider_ids: &BTreeSet<String>,
    context: &str,
    provider_id: &str,
) {
    if !known_provider_ids.contains(provider_id) {
        errors.push(format!(
            "{context} references provider_id `{provider_id}` but [gate_providers.{provider_id}] is not configured"
        ));
    }
}
