//! Per-provider binding root for bolt-v3 venue config block shapes
//! and per-venue startup-validation policy.
//!
//! Core config in `crate::bolt_v3_config` owns the root and strategy
//! envelopes plus minimal dispatch identifiers such as
//! `VenueKind::Polymarket` / `VenueKind::Binance`; the concrete
//! `[venues.<name>.{data,execution,secrets}]` block shapes themselves
//! live in per-provider binding modules under this root. This namespace
//! is the family-agnostic entry point: today it exposes `polymarket`
//! and `binance`, and a future provider adds its own per-provider
//! module here without changes to core config.
//!
//! This module also owns the family-agnostic dispatch surface that
//! core startup validation in `crate::bolt_v3_validate` calls into:
//! every `[venues.<id>]` block is routed here, the venue-kind
//! discriminator is read once, and the matching per-provider
//! validator owns the rest of the structural venue-shape rules.
//! Provider-neutral helpers used by more than one provider validator
//! (today: `crate::bolt_v3_validate::validate_ssm_parameter_path`)
//! stay in core and are called from the per-provider modules.

pub mod binance;
pub mod polymarket;

use crate::bolt_v3_config::{VenueBlock, VenueKind};

/// Family-agnostic surface read by core startup validation. Routes
/// each venue block to its per-provider validator based on
/// `VenueKind`. Returns the full error list for the venue block.
pub fn validate_venue_block(key: &str, venue: &VenueBlock) -> Vec<String> {
    match venue.kind {
        VenueKind::Polymarket => polymarket::validate_venue(key, venue),
        VenueKind::Binance => binance::validate_venue(key, venue),
    }
}
