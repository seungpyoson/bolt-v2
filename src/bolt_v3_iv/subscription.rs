use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{selector::IvSelector, types::IvSourceKind};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvProfileSubscriptionConfig {
    pub profile_id: String,
    pub sources: Vec<IvSourceSubscriptionConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvSourceSubscriptionConfig {
    pub source_id: String,
    pub source_kind: IvSourceKind,
    pub client_id: String,
    pub selector: IvSelector,
    pub params: toml::Value,
    pub subscription_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvSubscriptionLifecycle {
    Start,
    Stop,
    Reload,
    Unsubscribe,
    SourceRemoval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvRuntimeOperation {
    SubscribeOptionGreeks,
    UnsubscribeOptionGreeks,
    SubscribeOptionChain,
    UnsubscribeOptionChain,
    SubscribeAggregateGreeks,
    UnsubscribeAggregateGreeks,
    SubscribeCustomData,
    UnsubscribeCustomData,
    RemoveSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvNtSubscriptionKind {
    OptionGreeks,
    OptionChain,
    AggregateGreeksTopic,
    CustomData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvSubscriptionPlan {
    pub profile_id: String,
    pub source_id: String,
    pub lifecycle: IvSubscriptionLifecycle,
    pub operation: IvRuntimeOperation,
    pub nt_source_kind: IvNtSubscriptionKind,
    pub client_id: String,
    pub selector: IvSelector,
    pub params: toml::Value,
    pub subscription_generation: u64,
}

impl IvSubscriptionPlan {
    pub fn from_source(
        profile_id: &str,
        source: &IvSourceSubscriptionConfig,
        lifecycle: IvSubscriptionLifecycle,
        operation: IvRuntimeOperation,
        nt_source_kind: IvNtSubscriptionKind,
    ) -> Self {
        Self {
            profile_id: profile_id.to_string(),
            source_id: source.source_id.clone(),
            lifecycle,
            operation,
            nt_source_kind,
            client_id: source.client_id.clone(),
            selector: source.selector.clone(),
            params: source.params.clone(),
            subscription_generation: source.subscription_generation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvSubscriptionError {
    DuplicateSourceId {
        source_id: String,
    },
    SelectorKindMismatch {
        source_id: String,
        source_kind: IvSourceKind,
    },
}

pub fn plan_profile_start(
    profile: &IvProfileSubscriptionConfig,
) -> Result<Vec<IvSubscriptionPlan>, IvSubscriptionError> {
    validate_profile(profile)?;

    profile
        .sources
        .iter()
        .map(|source| {
            let (operation, nt_source_kind) = subscribe_mapping(source.source_kind);
            Ok(IvSubscriptionPlan::from_source(
                &profile.profile_id,
                source,
                IvSubscriptionLifecycle::Start,
                operation,
                nt_source_kind,
            ))
        })
        .collect()
}

pub fn plan_profile_stop(
    profile: &IvProfileSubscriptionConfig,
) -> Result<Vec<IvSubscriptionPlan>, IvSubscriptionError> {
    validate_profile(profile)?;

    profile
        .sources
        .iter()
        .map(|source| {
            let (operation, nt_source_kind) = unsubscribe_mapping(source.source_kind);
            Ok(IvSubscriptionPlan::from_source(
                &profile.profile_id,
                source,
                IvSubscriptionLifecycle::Stop,
                operation,
                nt_source_kind,
            ))
        })
        .collect()
}

pub fn plan_profile_reload(
    current: &IvProfileSubscriptionConfig,
    next: &IvProfileSubscriptionConfig,
) -> Result<Vec<IvSubscriptionPlan>, IvSubscriptionError> {
    validate_profile(current)?;
    validate_profile(next)?;

    let next_sources = source_map(next);
    let current_sources = source_map(current);
    let mut plans = Vec::new();

    for current_source in &current.sources {
        if let Some(next_source) = next_sources.get(&current_source.source_id) {
            if current_source != *next_source {
                let (unsubscribe_operation, nt_source_kind) =
                    unsubscribe_mapping(current_source.source_kind);
                plans.push(IvSubscriptionPlan::from_source(
                    &current.profile_id,
                    current_source,
                    IvSubscriptionLifecycle::Reload,
                    unsubscribe_operation,
                    nt_source_kind,
                ));

                let (subscribe_operation, nt_source_kind) =
                    subscribe_mapping(next_source.source_kind);
                plans.push(IvSubscriptionPlan::from_source(
                    &next.profile_id,
                    next_source,
                    IvSubscriptionLifecycle::Reload,
                    subscribe_operation,
                    nt_source_kind,
                ));
            }
        } else {
            let (unsubscribe_operation, nt_source_kind) =
                unsubscribe_mapping(current_source.source_kind);
            plans.push(IvSubscriptionPlan::from_source(
                &current.profile_id,
                current_source,
                IvSubscriptionLifecycle::SourceRemoval,
                unsubscribe_operation,
                nt_source_kind,
            ));
            plans.push(IvSubscriptionPlan::from_source(
                &current.profile_id,
                current_source,
                IvSubscriptionLifecycle::SourceRemoval,
                IvRuntimeOperation::RemoveSource,
                nt_source_kind,
            ));
        }
    }

    for next_source in &next.sources {
        if !current_sources.contains_key(&next_source.source_id) {
            let (operation, nt_source_kind) = subscribe_mapping(next_source.source_kind);
            plans.push(IvSubscriptionPlan::from_source(
                &next.profile_id,
                next_source,
                IvSubscriptionLifecycle::Start,
                operation,
                nt_source_kind,
            ));
        }
    }

    Ok(plans)
}

fn validate_profile(profile: &IvProfileSubscriptionConfig) -> Result<(), IvSubscriptionError> {
    let mut seen = BTreeSet::new();

    for source in &profile.sources {
        if !seen.insert(source.source_id.clone()) {
            return Err(IvSubscriptionError::DuplicateSourceId {
                source_id: source.source_id.clone(),
            });
        }

        if !selector_matches_source_kind(source.source_kind, &source.selector) {
            return Err(IvSubscriptionError::SelectorKindMismatch {
                source_id: source.source_id.clone(),
                source_kind: source.source_kind,
            });
        }
    }

    Ok(())
}

fn source_map(
    profile: &IvProfileSubscriptionConfig,
) -> BTreeMap<String, &IvSourceSubscriptionConfig> {
    profile
        .sources
        .iter()
        .map(|source| (source.source_id.clone(), source))
        .collect()
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

fn subscribe_mapping(source_kind: IvSourceKind) -> (IvRuntimeOperation, IvNtSubscriptionKind) {
    match source_kind {
        IvSourceKind::OptionGreeks => (
            IvRuntimeOperation::SubscribeOptionGreeks,
            IvNtSubscriptionKind::OptionGreeks,
        ),
        IvSourceKind::OptionChain => (
            IvRuntimeOperation::SubscribeOptionChain,
            IvNtSubscriptionKind::OptionChain,
        ),
        IvSourceKind::AggregateGreeks => (
            IvRuntimeOperation::SubscribeAggregateGreeks,
            IvNtSubscriptionKind::AggregateGreeksTopic,
        ),
        IvSourceKind::CustomImpliedVolatility => (
            IvRuntimeOperation::SubscribeCustomData,
            IvNtSubscriptionKind::CustomData,
        ),
    }
}

fn unsubscribe_mapping(source_kind: IvSourceKind) -> (IvRuntimeOperation, IvNtSubscriptionKind) {
    match source_kind {
        IvSourceKind::OptionGreeks => (
            IvRuntimeOperation::UnsubscribeOptionGreeks,
            IvNtSubscriptionKind::OptionGreeks,
        ),
        IvSourceKind::OptionChain => (
            IvRuntimeOperation::UnsubscribeOptionChain,
            IvNtSubscriptionKind::OptionChain,
        ),
        IvSourceKind::AggregateGreeks => (
            IvRuntimeOperation::UnsubscribeAggregateGreeks,
            IvNtSubscriptionKind::AggregateGreeksTopic,
        ),
        IvSourceKind::CustomImpliedVolatility => (
            IvRuntimeOperation::UnsubscribeCustomData,
            IvNtSubscriptionKind::CustomData,
        ),
    }
}
