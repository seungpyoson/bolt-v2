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

use crate::bolt_v3_config::LoadedBoltV3Config;

pub use updown::{BoltV3MarketIdentityError, MarketIdentityPlan};

pub fn plan_market_identity(
    loaded: &LoadedBoltV3Config,
) -> Result<MarketIdentityPlan, BoltV3MarketIdentityError> {
    updown::plan_market_identity(loaded)
}

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
    rotating_market_family: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum MarketSelectionTypeDispatch {
    RotatingMarket,
    FixedInstrument,
}

#[derive(Debug, Clone, Deserialize)]
struct TargetSelectionDispatch {
    market_selection_type: MarketSelectionTypeDispatch,
}

pub struct MarketFamilyValidationBinding {
    pub key: &'static str,
    pub validate_target: fn(&str, &toml::Value) -> Vec<String>,
}

const VALIDATION_BINDINGS: &[MarketFamilyValidationBinding] = &[MarketFamilyValidationBinding {
    key: updown::KEY,
    validate_target: updown::validate_target_block,
}];

pub fn validation_bindings() -> &'static [MarketFamilyValidationBinding] {
    VALIDATION_BINDINGS
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
    let selection: TargetSelectionDispatch = match target.clone().try_into() {
        Ok(value) => value,
        Err(error) => {
            return (metadata, vec![format!("{context}: target: {error}")]);
        }
    };
    if selection.market_selection_type == MarketSelectionTypeDispatch::FixedInstrument {
        return (
            metadata,
            vec![format!(
                "{context}: target.market_selection_type `fixed_instrument` is reserved but not supported by this build"
            )],
        );
    }
    let dispatch: TargetFamilyDispatch = match target.clone().try_into() {
        Ok(value) => value,
        Err(error) => {
            return (metadata, vec![format!("{context}: target: {error}")]);
        }
    };
    let errors = match validation_bindings()
        .iter()
        .find(|binding| binding.key == dispatch.rotating_market_family)
    {
        Some(binding) => (binding.validate_target)(context, target),
        None => vec![format!(
            "{context}: target.rotating_market_family `{}` is not supported by this build",
            dispatch.rotating_market_family
        )],
    };
    (metadata, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_value(source: &str) -> toml::Value {
        toml::from_str(source).expect("target TOML should parse")
    }

    #[test]
    fn fixed_instrument_selection_type_fails_with_explicit_unsupported_error() {
        let target = target_value(
            r#"
configured_target_id = "fixed_eth_yes"
market_selection_type = "fixed_instrument"
"#,
        );

        let (_metadata, errors) = validate_strategy_target("strategy", &target);
        assert!(
            errors.iter().any(|message| message.contains(
                "target.market_selection_type `fixed_instrument` is reserved but not supported by this build"
            )),
            "fixed_instrument must fail as explicit unsupported market_selection_type, got: {errors:#?}"
        );
    }
}
