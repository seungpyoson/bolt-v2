//! Generic strategy registration boundary for Bolt-v3.
//!
//! This module adapts validated bolt-v3 strategy envelopes to an injected
//! registration binding. Concrete strategy builders live outside this core
//! boundary so the live-node build path can stay strategy-agnostic.

use std::sync::Arc;

use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy, StrategyArchetypeKey},
    bolt_v3_secrets::ResolvedBoltV3Secrets,
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
};

#[derive(Clone, Copy)]
pub struct StrategyRuntimeBinding {
    pub key: &'static str,
    pub register: for<'a> fn(
        &mut LiveNode,
        StrategyRegistrationContext<'a>,
    ) -> Result<StrategyId, BoltV3StrategyRegistrationError>,
}

#[derive(Clone)]
pub struct StrategyRegistrationContext<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy: &'a LoadedStrategy,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub submit_admission: Arc<BoltV3SubmitAdmissionState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3RegisteredStrategy {
    pub strategy_instance_id: String,
    pub strategy_archetype: StrategyArchetypeKey,
    pub registered_strategy_id: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoltV3StrategyRegistrationSummary {
    pub registered: Vec<BoltV3RegisteredStrategy>,
}

#[derive(Debug)]
pub enum BoltV3StrategyRegistrationError {
    UnsupportedStrategy {
        strategy_archetype: String,
    },
    Binding {
        strategy_instance_id: String,
        strategy_archetype: String,
        message: String,
    },
}

impl std::fmt::Display for BoltV3StrategyRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedStrategy { strategy_archetype } => write!(
                f,
                "unsupported bolt-v3 strategy archetype `{strategy_archetype}`"
            ),
            Self::Binding {
                strategy_instance_id,
                strategy_archetype,
                message,
            } => write!(
                f,
                "strategies.{strategy_instance_id} ({strategy_archetype}) registration failed: {message}"
            ),
        }
    }
}

impl std::error::Error for BoltV3StrategyRegistrationError {}

pub fn register_bolt_v3_strategies_with<F>(
    loaded: &LoadedBoltV3Config,
    mut register: F,
) -> Result<BoltV3StrategyRegistrationSummary, BoltV3StrategyRegistrationError>
where
    F: FnMut(&LoadedStrategy) -> Result<String, BoltV3StrategyRegistrationError>,
{
    let mut summary = BoltV3StrategyRegistrationSummary::default();

    for strategy in &loaded.strategies {
        let registered_strategy_id = register(strategy)?;

        summary.registered.push(BoltV3RegisteredStrategy {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            strategy_archetype: strategy.config.strategy_archetype.clone(),
            registered_strategy_id,
        });
    }

    Ok(summary)
}

pub fn register_bolt_v3_strategies_on_node_with<F>(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    mut register: F,
) -> Result<BoltV3StrategyRegistrationSummary, BoltV3StrategyRegistrationError>
where
    F: FnMut(&mut LiveNode, &LoadedStrategy) -> Result<StrategyId, BoltV3StrategyRegistrationError>,
{
    let mut summary = BoltV3StrategyRegistrationSummary::default();

    for strategy in &loaded.strategies {
        let registered_strategy_id = register(node, strategy)?;
        summary.registered.push(BoltV3RegisteredStrategy {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            strategy_archetype: strategy.config.strategy_archetype.clone(),
            registered_strategy_id: registered_strategy_id.to_string(),
        });
    }

    Ok(summary)
}

pub fn register_bolt_v3_strategies_on_node_with_bindings(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    bindings: &[StrategyRuntimeBinding],
) -> Result<BoltV3StrategyRegistrationSummary, BoltV3StrategyRegistrationError> {
    let mut summary = BoltV3StrategyRegistrationSummary::default();

    for strategy in &loaded.strategies {
        let binding = bindings
            .iter()
            .find(|binding| binding.key == strategy.config.strategy_archetype.as_str())
            .ok_or_else(|| BoltV3StrategyRegistrationError::UnsupportedStrategy {
                strategy_archetype: strategy.config.strategy_archetype.as_str().to_string(),
            })?;
        let registered_strategy_id = (binding.register)(
            node,
            StrategyRegistrationContext {
                loaded,
                strategy,
                resolved,
                submit_admission: submit_admission.clone(),
            },
        )?;
        summary.registered.push(BoltV3RegisteredStrategy {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            strategy_archetype: strategy.config.strategy_archetype.clone(),
            registered_strategy_id: registered_strategy_id.to_string(),
        });
    }

    Ok(summary)
}
