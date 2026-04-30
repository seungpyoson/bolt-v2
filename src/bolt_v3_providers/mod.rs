//! Per-provider binding root for bolt-v3 venue config block shapes
//! and per-venue startup-validation policy.
//!
//! Core config in `crate::bolt_v3_config` owns the root and strategy
//! envelopes plus raw provider keys. Concrete provider key literals and
//! `[venues.<name>.{data,execution,secrets}]` block shapes live in
//! per-provider binding modules under this root.
//!
//! This module also owns the family-agnostic dispatch surface that
//! core startup validation in `crate::bolt_v3_validate` calls into:
//! every `[venues.<id>]` block is routed here, the provider key is read
//! once, and the matching per-provider
//! validator owns the rest of the structural venue-shape rules.
//! Provider-neutral helpers used by more than one provider validator
//! (today: `crate::bolt_v3_validate::validate_ssm_parameter_path`)
//! stay in core and are called from the per-provider modules.

pub mod binance;
pub mod polymarket;

use crate::bolt_v3_config::VenueBlock;

pub struct ProviderValidationBinding {
    pub key: &'static str,
    pub validate_venue: fn(&str, &VenueBlock) -> Vec<String>,
    pub credential_log_modules: &'static [&'static str],
}

const VALIDATION_BINDINGS: &[ProviderValidationBinding] = &[
    ProviderValidationBinding {
        key: polymarket::KEY,
        validate_venue: polymarket::validate_venue,
        credential_log_modules: polymarket::CREDENTIAL_LOG_MODULES,
    },
    ProviderValidationBinding {
        key: binance::KEY,
        validate_venue: binance::validate_venue,
        credential_log_modules: binance::CREDENTIAL_LOG_MODULES,
    },
];

pub fn validation_bindings() -> &'static [ProviderValidationBinding] {
    VALIDATION_BINDINGS
}

/// Provider-owned NT adapter modules whose info logs can expose
/// credential metadata. The live-node builder consumes this provider
/// binding surface to install `WARN` module filters without hardcoding
/// concrete provider module paths in the live-node assembly layer.
pub fn credential_log_modules() -> impl Iterator<Item = &'static str> {
    validation_bindings()
        .iter()
        .flat_map(|binding| binding.credential_log_modules.iter().copied())
}

/// Family-agnostic surface read by core startup validation. Routes
/// each venue block to its per-provider validator based on provider
/// key. Returns the full error list for the venue block.
pub fn validate_venue_block(key: &str, venue: &VenueBlock) -> Vec<String> {
    match validation_bindings()
        .iter()
        .find(|binding| binding.key == venue.kind.as_str())
    {
        Some(binding) => (binding.validate_venue)(key, venue),
        None => vec![format!(
            "venues.{key}.kind `{}` is not supported by this build",
            venue.kind.as_str()
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_log_modules_are_provider_owned() {
        let modules: Vec<_> = credential_log_modules().collect();
        let expected: Vec<_> = validation_bindings()
            .iter()
            .flat_map(|binding| binding.credential_log_modules.iter().copied())
            .collect();
        assert_eq!(modules, expected);

        let polymarket = validation_bindings()
            .iter()
            .find(|binding| binding.key == polymarket::KEY)
            .expect("Polymarket binding must be registered");
        assert_eq!(
            polymarket.credential_log_modules,
            polymarket::CREDENTIAL_LOG_MODULES
        );

        let binance = validation_bindings()
            .iter()
            .find(|binding| binding.key == binance::KEY)
            .expect("Binance binding must be registered");
        assert_eq!(
            binance.credential_log_modules,
            binance::CREDENTIAL_LOG_MODULES
        );
    }
}
