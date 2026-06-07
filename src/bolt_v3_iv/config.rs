use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    selector::IvSelector,
    subscription::{IvProfileSubscriptionConfig, IvSourceSubscriptionConfig, plan_profile_start},
    types::{IvProductKind, IvSourceKind},
};

pub const SUPPORTED_IV_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvRootConfig {
    pub schema_version: u32,
    pub profiles: Vec<IvProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvProfile {
    pub profile_id: String,
    pub enabled_products: BTreeSet<IvProductKind>,
    pub max_raw_events: usize,
    pub max_indexed_points: usize,
    pub max_smiles: usize,
    pub max_surfaces: usize,
    pub max_source_health_events: usize,
    pub sources: Vec<IvSourceConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvSourceConfig {
    pub source_id: String,
    pub source_kind: IvSourceKind,
    pub client_id: String,
    pub accepted_conventions: BTreeSet<String>,
    pub selector: IvSelector,
    pub params: toml::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvConfigError {
    Parse(String),
    Validation(Vec<String>),
}

pub fn load_iv_config_from_toml(text: &str) -> Result<IvRootConfig, IvConfigError> {
    let config = toml::from_str::<IvRootConfig>(text)
        .map_err(|error| IvConfigError::Parse(error.to_string()))?;
    let errors = validate_iv_root_config(&config);

    if errors.is_empty() {
        Ok(config)
    } else {
        Err(IvConfigError::Validation(errors))
    }
}

pub fn validate_iv_root_config(config: &IvRootConfig) -> Vec<String> {
    let mut errors = Vec::new();

    if config.schema_version != SUPPORTED_IV_SCHEMA_VERSION {
        errors.push(format!(
            "iv.schema_version={} is unsupported by this build (only {} is currently supported)",
            config.schema_version, SUPPORTED_IV_SCHEMA_VERSION
        ));
    }
    if config.profiles.is_empty() {
        errors.push("iv.profiles must contain at least one profile".to_string());
    }

    for profile in &config.profiles {
        errors.extend(validate_profile(profile));
    }

    errors
}

impl IvProfile {
    pub fn subscription_config(&self) -> IvProfileSubscriptionConfig {
        IvProfileSubscriptionConfig {
            profile_id: self.profile_id.clone(),
            sources: self
                .sources
                .iter()
                .map(|source| IvSourceSubscriptionConfig {
                    source_id: source.source_id.clone(),
                    source_kind: source.source_kind,
                    client_id: source.client_id.clone(),
                    selector: source.selector.clone(),
                    params: source.params.clone(),
                    subscription_generation: 0,
                })
                .collect(),
        }
    }
}

fn validate_profile(profile: &IvProfile) -> Vec<String> {
    let mut errors = Vec::new();
    let profile_context = format!("iv.profiles.{}", profile.profile_id);

    if profile.profile_id.trim().is_empty() {
        errors.push("iv.profiles.profile_id must be non-empty".to_string());
    }
    if profile.enabled_products.is_empty() {
        errors.push(format!(
            "{profile_context}.enabled_products must be non-empty"
        ));
    }
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_raw_events",
        profile.max_raw_events,
    );
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_indexed_points",
        profile.max_indexed_points,
    );
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_smiles",
        profile.max_smiles,
    );
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_surfaces",
        profile.max_surfaces,
    );
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_source_health_events",
        profile.max_source_health_events,
    );
    if profile.sources.is_empty() {
        errors.push(format!(
            "{profile_context}.sources must contain at least one source"
        ));
    }

    let mut seen_sources = BTreeSet::new();
    for source in &profile.sources {
        let source_context = format!("{profile_context}.sources.{}", source.source_id);
        if !seen_sources.insert(source.source_id.clone()) {
            errors.push(format!("{source_context}.source_id is duplicated"));
        }
        errors.extend(validate_source(&source_context, source));
    }

    if let Err(error) = plan_profile_start(&profile.subscription_config()) {
        errors.push(format!(
            "{profile_context}.subscription planning rejected: {error:?}"
        ));
    }

    errors
}

fn validate_positive_bound(errors: &mut Vec<String>, context: &str, field: &str, value: usize) {
    if value == 0 {
        errors.push(format!("{context}.{field} must be positive"));
    }
}

fn validate_source(context: &str, source: &IvSourceConfig) -> Vec<String> {
    let mut errors = Vec::new();

    if source.source_id.trim().is_empty() {
        errors.push(format!("{context}.source_id must be non-empty"));
    }
    if source.client_id.trim().is_empty() {
        errors.push(format!("{context}.client_id must be non-empty"));
    }
    if source.accepted_conventions.is_empty() {
        errors.push(format!("{context}.accepted_conventions must be non-empty"));
    }
    if !selector_matches_source_kind(source.source_kind, &source.selector) {
        errors.push(format!(
            "{context}.selector variant must match source_kind {:?}",
            source.source_kind
        ));
    }
    errors.extend(validate_selector_not_empty(context, &source.selector));

    errors
}

fn selector_matches_source_kind(source_kind: IvSourceKind, selector: &IvSelector) -> bool {
    matches!(
        (source_kind, selector),
        (
            IvSourceKind::OptionGreeks,
            IvSelector::SourceOptionGreeks { .. }
        ) | (
            IvSourceKind::OptionChain,
            IvSelector::SourceOptionChain { .. }
        ) | (
            IvSourceKind::AggregateGreeks,
            IvSelector::SourceAggregateGreeks { .. }
        ) | (
            IvSourceKind::CustomImpliedVolatility,
            IvSelector::SourceCustomImpliedVolatility { .. }
        )
    )
}

fn validate_selector_not_empty(context: &str, selector: &IvSelector) -> Vec<String> {
    match selector {
        IvSelector::SourceOptionGreeks { instrument_ids, .. } if instrument_ids.is_empty() => {
            vec![format!(
                "{context}.selector.instrument_ids must be non-empty"
            )]
        }
        IvSelector::SourceOptionChain {
            series_ids,
            strike_range_policy,
            ..
        } if series_ids.is_empty() || strike_range_policy.trim().is_empty() => {
            vec![format!(
                "{context}.selector.series_ids and strike_range_policy must be non-empty"
            )]
        }
        IvSelector::SourceAggregateGreeks {
            aggregate_key,
            underlying_selectors,
            ..
        } if aggregate_key.trim().is_empty() || underlying_selectors.is_empty() => {
            vec![format!(
                "{context}.selector.aggregate_key and underlying_selectors must be non-empty"
            )]
        }
        IvSelector::SourceCustomImpliedVolatility {
            custom_iv_data_type,
            custom_iv_data_fields,
            ..
        } if custom_iv_data_type.trim().is_empty() || custom_iv_data_fields.is_empty() => {
            vec![format!(
                "{context}.selector.custom_iv_data_type and custom_iv_data_fields must be non-empty"
            )]
        }
        _ => Vec::new(),
    }
}
