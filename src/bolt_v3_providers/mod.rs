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
}

const VALIDATION_BINDINGS: &[ProviderValidationBinding] = &[
    ProviderValidationBinding {
        key: polymarket::KEY,
        validate_venue: polymarket::validate_venue,
    },
    ProviderValidationBinding {
        key: binance::KEY,
        validate_venue: binance::validate_venue,
    },
];

pub fn validation_bindings() -> &'static [ProviderValidationBinding] {
    VALIDATION_BINDINGS
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
