//! Rotating-market target parsing for bolt-v3 strategy config.
//!
//! Each supported `target.rotating_market_family` has a module here
//! that owns its typed `[target]` fields, cadence checks, slug
//! construction, and instrument-filter errors.

pub mod updown;

use serde::Deserialize;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_instrument_filters::{
        InstrumentFilterConfig, InstrumentFilterError, InstrumentFilterTarget,
    },
};
use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};

/// Target metadata read by startup validation before dispatching to a
/// `target.rotating_market_family` validator.
#[derive(Debug, Clone, Deserialize)]
pub struct TargetMetadata {
    pub configured_target_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TargetFamilyDispatch {
    rotating_market_family: String,
}

pub struct MarketFamilyValidationBinding {
    pub key: &'static str,
    pub validate_target: fn(&str, &toml::Value) -> Vec<String>,
    pub instrument_filter_targets:
        fn(&LoadedBoltV3Config) -> Result<Vec<InstrumentFilterTarget>, InstrumentFilterError>,
    pub target_runtime_fields: fn(&toml::Value) -> Result<TargetRuntimeFields, String>,
    pub select_binary_option_market:
        fn(MarketSelectionTarget<'_>, &[InstrumentAny], u64) -> Option<SelectedBinaryOptionMarket>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketSelectionTarget<'a> {
    pub family_key: &'a str,
    pub underlying_asset: &'a str,
    pub cadence_seconds: i64,
    pub cadence_slug_token: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedBinaryOptionMarket {
    pub market_id: String,
    pub instrument_id: InstrumentId,
    pub up_instrument_id: InstrumentId,
    pub down_instrument_id: InstrumentId,
    pub start_timestamp_milliseconds: u64,
    pub seconds_to_end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRuntimeFields {
    pub configured_target_id: String,
    pub target_kind: String,
    pub rotating_market_family: String,
    pub underlying_asset: String,
    pub cadence_seconds: i64,
    pub cadence_seconds_source_field: &'static str,
    pub cadence_slug_token: String,
    pub market_selection_rule: String,
    pub retry_interval_seconds: u64,
    pub blocked_after_seconds: u64,
}

const VALIDATION_BINDINGS: &[MarketFamilyValidationBinding] = &[MarketFamilyValidationBinding {
    key: updown::KEY,
    validate_target: updown::validate_target_block,
    instrument_filter_targets: updown::instrument_filter_targets,
    target_runtime_fields: updown::target_runtime_fields,
    select_binary_option_market: updown::select_binary_option_market,
}];

pub fn validation_bindings() -> &'static [MarketFamilyValidationBinding] {
    VALIDATION_BINDINGS
}

pub fn instrument_filters_from_config(
    loaded: &LoadedBoltV3Config,
) -> Result<InstrumentFilterConfig, InstrumentFilterError> {
    instrument_filters_from_config_with_bindings(loaded, validation_bindings())
}

pub fn instrument_filters_from_config_with_bindings(
    loaded: &LoadedBoltV3Config,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<InstrumentFilterConfig, InstrumentFilterError> {
    let mut targets = Vec::new();
    for binding in bindings {
        targets.extend((binding.instrument_filter_targets)(loaded)?);
    }
    Ok(InstrumentFilterConfig::new(targets))
}

pub fn target_runtime_fields_from_target(
    target: &toml::Value,
) -> Result<TargetRuntimeFields, String> {
    target_runtime_fields_from_target_with_bindings(target, validation_bindings())
}

pub fn target_runtime_fields_from_target_with_bindings(
    target: &toml::Value,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<TargetRuntimeFields, String> {
    let dispatch: TargetFamilyDispatch = target
        .clone()
        .try_into()
        .map_err(|error| format!("target: {error}"))?;
    bindings
        .iter()
        .find(|binding| binding.key == dispatch.rotating_market_family)
        .ok_or_else(|| {
            format!(
                "target.rotating_market_family `{}` is not supported by this build",
                dispatch.rotating_market_family
            )
        })
        .and_then(|binding| (binding.target_runtime_fields)(target))
}

pub fn select_binary_option_market_from_target(
    target: MarketSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedBinaryOptionMarket> {
    select_binary_option_market_from_target_with_bindings(
        target,
        instruments,
        now_milliseconds,
        validation_bindings(),
    )
}

pub fn select_binary_option_market_from_target_with_bindings(
    target: MarketSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
    bindings: &[MarketFamilyValidationBinding],
) -> Option<SelectedBinaryOptionMarket> {
    bindings
        .iter()
        .find(|binding| binding.key == target.family_key)
        .and_then(|binding| {
            (binding.select_binary_option_market)(target, instruments, now_milliseconds)
        })
}

impl From<updown::BoltV3InstrumentFilterError> for InstrumentFilterError {
    fn from(error: updown::BoltV3InstrumentFilterError) -> Self {
        Self::new(error.to_string())
    }
}

/// Target validation entry point used by core startup validation.
/// Returns `(metadata, errors)`: the metadata is `None` when the raw
/// `[target]` value cannot even produce a `configured_target_id` (in
/// which case the family-specific validator's full error set still
/// surfaces in `errors`).
pub fn validate_strategy_target(
    context: &str,
    target: &toml::Value,
) -> (Option<TargetMetadata>, Vec<String>) {
    validate_strategy_target_with_bindings(context, target, validation_bindings())
}

pub fn validate_strategy_target_with_bindings(
    context: &str,
    target: &toml::Value,
    bindings: &[MarketFamilyValidationBinding],
) -> (Option<TargetMetadata>, Vec<String>) {
    let metadata = target.clone().try_into::<TargetMetadata>().ok();
    let dispatch: TargetFamilyDispatch = match target.clone().try_into() {
        Ok(value) => value,
        Err(error) => {
            return (metadata, vec![format!("{context}: target: {error}")]);
        }
    };
    let errors = match bindings
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

    fn fake_validate_target(_context: &str, _target: &toml::Value) -> Vec<String> {
        Vec::new()
    }

    const FAKE_FAMILY_BINDINGS: &[MarketFamilyValidationBinding] =
        &[MarketFamilyValidationBinding {
            key: "fixture_family",
            validate_target: fake_validate_target,
            instrument_filter_targets: fake_instrument_filter_targets,
            target_runtime_fields: fake_target_runtime_fields,
            select_binary_option_market: fake_select_binary_option_market,
        }];

    fn fake_instrument_filter_targets(
        loaded: &LoadedBoltV3Config,
    ) -> Result<Vec<InstrumentFilterTarget>, InstrumentFilterError> {
        Err(InstrumentFilterError::new(format!(
            "fixture_family binding invoked for {}",
            loaded.root.trader_id
        )))
    }

    fn fake_target_runtime_fields(_target: &toml::Value) -> Result<TargetRuntimeFields, String> {
        Err("fixture_family target runtime binding invoked".to_string())
    }

    fn fake_select_binary_option_market(
        _target: MarketSelectionTarget<'_>,
        _instruments: &[InstrumentAny],
        _now_milliseconds: u64,
    ) -> Option<SelectedBinaryOptionMarket> {
        Some(SelectedBinaryOptionMarket {
            market_id: "fixture-market".to_string(),
            instrument_id: InstrumentId::from("fixture-market.FIXTURE"),
            up_instrument_id: InstrumentId::from("fixture-up.FIXTURE"),
            down_instrument_id: InstrumentId::from("fixture-down.FIXTURE"),
            start_timestamp_milliseconds: 1_000,
            seconds_to_end: 60,
        })
    }

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        let root: crate::bolt_v3_config::BoltV3RootConfig =
            toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml")).unwrap();
        LoadedBoltV3Config {
            root_path: std::path::PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root,
            strategies: Vec::new(),
        }
    }

    #[test]
    fn validation_can_use_injected_family_binding_without_editing_production_registry() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "fixture_family"
        }
        .into();

        let (_, production_errors) = validate_strategy_target("strategy `fixture`", &target);
        assert!(
            production_errors
                .iter()
                .any(|message| message.contains("not supported by this build")),
            "production registry should not know the test family: {production_errors:?}"
        );

        let (_, injected_errors) = validate_strategy_target_with_bindings(
            "strategy `fixture`",
            &target,
            FAKE_FAMILY_BINDINGS,
        );
        assert!(
            injected_errors.is_empty(),
            "injected family binding should own target dispatch: {injected_errors:?}"
        );
    }

    #[test]
    fn instrument_filters_use_injected_family_binding_without_parent_family_branch() {
        let loaded = fixture_loaded_config();
        let production_filters = instrument_filters_from_config(&loaded).unwrap();
        assert_eq!(
            production_filters.target_refs().count(),
            0,
            "fixture has no loaded strategies, so production registry should not emit filters"
        );

        let injected_error =
            instrument_filters_from_config_with_bindings(&loaded, FAKE_FAMILY_BINDINGS)
                .expect_err("fake binding should own this dispatch and return its error");
        assert_eq!(
            injected_error.to_string(),
            format!(
                "fixture_family binding invoked for {}",
                loaded.root.trader_id
            )
        );
    }

    #[test]
    fn target_runtime_fields_use_injected_family_binding_without_parent_family_branch() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "fixture_family"
        }
        .into();

        let production_error = target_runtime_fields_from_target(&target)
            .expect_err("production registry should not know the test family");
        assert!(
            production_error.contains("not supported by this build"),
            "production registry should not know the test family: {production_error}"
        );

        let injected_error =
            target_runtime_fields_from_target_with_bindings(&target, FAKE_FAMILY_BINDINGS)
                .expect_err("fake binding should own this dispatch and return its error");
        assert_eq!(
            injected_error,
            "fixture_family target runtime binding invoked"
        );
    }

    #[test]
    fn market_selection_uses_injected_family_binding_without_parent_family_branch() {
        let target = MarketSelectionTarget {
            family_key: "fixture_family",
            underlying_asset: "FIXTURE",
            cadence_seconds: 60,
            cadence_slug_token: "fixture",
        };

        assert!(
            select_binary_option_market_from_target(target, &[], 0).is_none(),
            "production registry should not know the test family"
        );

        let selected = select_binary_option_market_from_target_with_bindings(
            target,
            &[],
            0,
            FAKE_FAMILY_BINDINGS,
        )
        .expect("injected family binding should own market selection dispatch");

        assert_eq!(selected.market_id, "fixture-market");
    }
}
