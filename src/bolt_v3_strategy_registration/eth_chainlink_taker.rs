use nautilus_common::component::Component;
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;
use toml::{Value, map::Map};

use crate::{
    bolt_v3_config::{BoltV3StrategyConfig, LoadedStrategy},
    bolt_v3_providers::{self, polymarket::ResolvedBoltV3PolymarketSecrets},
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
    },
    strategies::{
        eth_chainlink_taker::{ETH_CHAINLINK_TAKER_KIND, EthChainlinkTakerBuilder},
        registry::StrategyBuildContext,
    },
};

const REFERENCE_PUBLISH_TOPIC_FIELD: &str = "reference_publish_topic";
const STRATEGY_ID_FIELD: &str = "strategy_id";
const CLIENT_ID_FIELD: &str = "client_id";

pub const STRATEGY_RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding {
    key: ETH_CHAINLINK_TAKER_KIND,
    register,
};

pub fn legacy_config_from_strategy(
    strategy: &BoltV3StrategyConfig,
) -> Result<Value, BoltV3StrategyRegistrationError> {
    let mut table = match strategy.parameters.clone() {
        Value::Table(table) => table,
        value => {
            return Err(BoltV3StrategyRegistrationError::InvalidParameters {
                reason: format!(
                    "strategy_archetype `{}` requires [parameters] to be a table, got {value:?}",
                    strategy.strategy_archetype.as_str()
                ),
            });
        }
    };

    require_reference_publish_topic(strategy, &mut table)?;
    table.insert(
        STRATEGY_ID_FIELD.to_string(),
        Value::String(strategy.strategy_instance_id.clone()),
    );
    table.insert(
        CLIENT_ID_FIELD.to_string(),
        Value::String(strategy.execution_client_id.clone()),
    );
    Ok(Value::Table(table))
}

fn register(
    node: &mut LiveNode,
    context: StrategyRegistrationContext<'_>,
) -> Result<StrategyId, BoltV3StrategyRegistrationError> {
    let raw = legacy_config_from_strategy(&context.strategy.config)?;
    let build_context = build_context(&context)?;
    let strategy =
        EthChainlinkTakerBuilder::build_concrete(&raw, &build_context).map_err(|source| {
            BoltV3StrategyRegistrationError::InvalidParameters {
                reason: format!(
                    "strategy `{}` failed to construct `EthChainlinkTaker`: {source}",
                    context.strategy.relative_path
                ),
            }
        })?;
    let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
    node.add_strategy(strategy)
        .map_err(|source| BoltV3StrategyRegistrationError::AddStrategy {
            strategy_file: context.strategy.relative_path.clone(),
            source,
        })?;
    Ok(strategy_id)
}

fn build_context(
    context: &StrategyRegistrationContext<'_>,
) -> Result<StrategyBuildContext, BoltV3StrategyRegistrationError> {
    let strategy = &context.strategy.config;
    let client_id = context
        .loaded
        .root
        .clients
        .get(&strategy.execution_client_id)
        .ok_or_else(|| BoltV3StrategyRegistrationError::MissingClient {
            strategy_file: context.strategy.relative_path.clone(),
            client_id: strategy.execution_client_id.clone(),
        })?;

    if client_id.venue.as_str() != bolt_v3_providers::polymarket::KEY {
        return Err(BoltV3StrategyRegistrationError::UnsupportedVenue {
            strategy_file: context.strategy.relative_path.clone(),
            client_id: strategy.execution_client_id.clone(),
            venue: client_id.venue.as_str().to_string(),
        });
    }

    let execution = client_id.execution.as_ref().ok_or_else(|| {
        BoltV3StrategyRegistrationError::MissingExecutionBlock {
            strategy_file: context.strategy.relative_path.clone(),
            client_id: strategy.execution_client_id.clone(),
        }
    })?;
    let secrets = context
        .resolved
        .get_as::<ResolvedBoltV3PolymarketSecrets>(&strategy.execution_client_id)
        .ok_or_else(|| BoltV3StrategyRegistrationError::MissingProviderSecrets {
            strategy_file: context.strategy.relative_path.clone(),
            client_id: strategy.execution_client_id.clone(),
        })?;
    let fee_provider = bolt_v3_providers::polymarket::build_fee_provider(
        execution,
        secrets,
        context.loaded.root.nautilus.timeout_connection_seconds,
    )
    .map_err(|source| BoltV3StrategyRegistrationError::FeeProviderBuild {
        strategy_file: context.strategy.relative_path.clone(),
        client_id: strategy.execution_client_id.clone(),
        source,
    })?;

    Ok(StrategyBuildContext {
        fee_provider,
        reference_publish_topic: reference_publish_topic(context.strategy)?,
    })
}

fn require_reference_publish_topic(
    strategy: &BoltV3StrategyConfig,
    table: &mut Map<String, Value>,
) -> Result<(), BoltV3StrategyRegistrationError> {
    match table.remove(REFERENCE_PUBLISH_TOPIC_FIELD) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(()),
        Some(Value::String(_)) => Err(BoltV3StrategyRegistrationError::InvalidParameters {
            reason: format!(
                "strategy_archetype `{}` requires parameters.{REFERENCE_PUBLISH_TOPIC_FIELD} to be non-empty",
                strategy.strategy_archetype.as_str()
            ),
        }),
        Some(value) => Err(BoltV3StrategyRegistrationError::InvalidParameters {
            reason: format!(
                "strategy_archetype `{}` requires parameters.{REFERENCE_PUBLISH_TOPIC_FIELD} to be a string, got {value:?}",
                strategy.strategy_archetype.as_str()
            ),
        }),
        None => Err(BoltV3StrategyRegistrationError::InvalidParameters {
            reason: format!(
                "strategy_archetype `{}` requires parameters.{REFERENCE_PUBLISH_TOPIC_FIELD}",
                strategy.strategy_archetype.as_str()
            ),
        }),
    }
}

fn reference_publish_topic(
    strategy: &LoadedStrategy,
) -> Result<String, BoltV3StrategyRegistrationError> {
    let Value::Table(table) = &strategy.config.parameters else {
        return Err(BoltV3StrategyRegistrationError::InvalidParameters {
            reason: format!(
                "strategy_archetype `{}` requires [parameters] to be a table",
                strategy.config.strategy_archetype.as_str()
            ),
        });
    };
    match table.get(REFERENCE_PUBLISH_TOPIC_FIELD) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        Some(Value::String(_)) => Err(BoltV3StrategyRegistrationError::InvalidParameters {
            reason: format!(
                "strategy `{}` requires parameters.{REFERENCE_PUBLISH_TOPIC_FIELD} to be non-empty",
                strategy.relative_path
            ),
        }),
        Some(value) => Err(BoltV3StrategyRegistrationError::InvalidParameters {
            reason: format!(
                "strategy `{}` requires parameters.{REFERENCE_PUBLISH_TOPIC_FIELD} to be a string, got {value:?}",
                strategy.relative_path
            ),
        }),
        None => Err(BoltV3StrategyRegistrationError::InvalidParameters {
            reason: format!(
                "strategy `{}` requires parameters.{REFERENCE_PUBLISH_TOPIC_FIELD}",
                strategy.relative_path
            ),
        }),
    }
}
