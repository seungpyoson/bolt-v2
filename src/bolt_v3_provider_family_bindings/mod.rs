//! Provider + market-family glue for bolt-v3.
//!
//! Provider bindings own venue/client mechanics. Market-family bindings
//! own target identity. This module owns the narrow cross-product
//! needed when one provider needs provider-shaped NT filters or
//! readiness checks for one market family.

pub mod polymarket_updown;

use std::sync::Arc;

use nautilus_polymarket::filters::InstrumentFilter;

use crate::{
    bolt_v3_adapters::BoltV3MarketSelectionNowFn,
    bolt_v3_market_families::{BoltV3MarketIdentityError, MarketIdentityPlan},
    bolt_v3_providers::{ProviderInstrumentReadinessContext, ProviderInstrumentReadinessFact},
};

pub struct ProviderFamilyFilterContext<'a> {
    pub client_id_key: &'a str,
    pub plan: &'a MarketIdentityPlan,
    pub clock: BoltV3MarketSelectionNowFn,
}

pub struct ProviderMarketFamilyBinding {
    pub provider_key: &'static str,
    pub family_key: &'static str,
    pub build_polymarket_filters:
        Option<for<'a> fn(ProviderFamilyFilterContext<'a>) -> Vec<Arc<dyn InstrumentFilter>>>,
    pub check_instrument_readiness: Option<
        for<'a> fn(
            ProviderInstrumentReadinessContext<'a>,
        )
            -> Result<Vec<ProviderInstrumentReadinessFact>, BoltV3MarketIdentityError>,
    >,
}

const BINDINGS: &[ProviderMarketFamilyBinding] = &[polymarket_updown::BINDING];

pub fn provider_supports_family(provider_key: &str, family_key: &str) -> bool {
    BINDINGS
        .iter()
        .any(|binding| binding.provider_key == provider_key && binding.family_key == family_key)
}

pub fn build_polymarket_filters_for_client(
    provider_key: &str,
    plan: &MarketIdentityPlan,
    client_id_key: &str,
    clock: BoltV3MarketSelectionNowFn,
) -> Vec<Arc<dyn InstrumentFilter>> {
    BINDINGS
        .iter()
        .filter(|binding| binding.provider_key == provider_key)
        .filter_map(|binding| binding.build_polymarket_filters)
        .flat_map(|build| {
            build(ProviderFamilyFilterContext {
                client_id_key,
                plan,
                clock: clock.clone(),
            })
        })
        .collect()
}

pub fn check_instrument_readiness(
    context: ProviderInstrumentReadinessContext<'_>,
) -> Result<Vec<ProviderInstrumentReadinessFact>, BoltV3MarketIdentityError> {
    let target_refs: Vec<_> = context
        .plan
        .client_id_target_refs()
        .filter(|target| target.client_id_key == context.client_id_key)
        .collect();
    if target_refs.is_empty() {
        return Ok(Vec::new());
    }

    let mut facts = Vec::new();
    let mut checked_families = Vec::<&'static str>::new();
    for target in target_refs {
        if checked_families.contains(&target.family_key) {
            continue;
        }
        checked_families.push(target.family_key);
        let Some(binding) = BINDINGS.iter().find(|binding| {
            binding.provider_key == context.venue_key && binding.family_key == target.family_key
        }) else {
            facts.push(ProviderInstrumentReadinessFact {
                client_id_key: context.client_id_key.to_string(),
                strategy_instance_id: target.strategy_instance_id.to_string(),
                configured_target_id: target.configured_target_id.to_string(),
                status: crate::bolt_v3_providers::ProviderInstrumentReadinessStatus::Blocked,
                detail: format!(
                    "venue `{}` has no instrument readiness binding for market family `{}`",
                    context.venue_key, target.family_key
                ),
            });
            continue;
        };
        let Some(check) = binding.check_instrument_readiness else {
            facts.push(ProviderInstrumentReadinessFact {
                client_id_key: context.client_id_key.to_string(),
                strategy_instance_id: target.strategy_instance_id.to_string(),
                configured_target_id: target.configured_target_id.to_string(),
                status: crate::bolt_v3_providers::ProviderInstrumentReadinessStatus::Blocked,
                detail: format!(
                    "venue `{}` has no instrument readiness check for market family `{}`",
                    context.venue_key, target.family_key
                ),
            });
            continue;
        };
        facts.extend(check(ProviderInstrumentReadinessContext { ..context })?);
    }
    Ok(facts)
}
