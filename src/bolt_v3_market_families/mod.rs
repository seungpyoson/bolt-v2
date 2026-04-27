//! Per-family market-identity binding modules for bolt-v3.
//!
//! The provider-neutral, family-agnostic boundary lives in
//! `crate::bolt_v3_market_identity`. Each market family this crate
//! supports has its own binding module here that owns the family-
//! specific identity surface (token-table lookup, period arithmetic,
//! market-id formatting, candidate selection, target projection,
//! family-specific error variants). New families plug in by adding a
//! sibling module rather than by editing the family-agnostic core.
//!
//! This module also owns the family-agnostic dispatch surface that
//! core startup validation in `crate::bolt_v3_validate` calls into:
//! the strategy envelope's raw `[target]` value is routed here, the
//! family discriminator is read once, and the matching per-family
//! validator owns the rest of the structural target-shape rules.

pub mod updown;

use serde::Deserialize;

use updown::RotatingMarketFamily;

/// Family-agnostic target-shape metadata read by core startup
/// validation for cross-family checks (today: uniqueness of
/// `configured_target_id` across configured strategies). Family-
/// specific fields are owned by the family binding modules; this
/// struct stays minimal so the dispatch layer never names a per-family
/// concept.
#[derive(Debug, Clone, Deserialize)]
pub struct TargetMetadata {
    pub configured_target_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TargetFamilyDispatch {
    rotating_market_family: RotatingMarketFamily,
}

/// Family-agnostic surface read by core startup validation.
/// Returns `(metadata, errors)`: the metadata is `None` when the raw
/// `[target]` value cannot even produce a `configured_target_id` (in
/// which case the family-specific validator's full error set still
/// surfaces in `errors`).
pub fn validate_strategy_target(
    context: &str,
    target: &toml::Value,
) -> (Option<TargetMetadata>, Vec<String>) {
    let metadata = target.clone().try_into::<TargetMetadata>().ok();
    let dispatch: TargetFamilyDispatch = match target.clone().try_into() {
        Ok(value) => value,
        Err(error) => {
            return (metadata, vec![format!("{context}: target: {error}")]);
        }
    };
    let errors = match dispatch.rotating_market_family {
        RotatingMarketFamily::Updown => updown::validate_target_block(context, target),
    };
    (metadata, errors)
}
