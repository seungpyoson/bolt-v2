use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy},
    bolt_v3_secrets::ResolvedBoltV3Secrets,
};

pub mod eth_chainlink_taker;

#[derive(Clone, Copy)]
pub struct StrategyRuntimeBinding {
    pub key: &'static str,
    pub register: for<'a> fn(
        &mut LiveNode,
        StrategyRegistrationContext<'a>,
    ) -> Result<StrategyId, BoltV3StrategyRegistrationError>,
}

pub struct StrategyRegistrationContext<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub strategy: &'a LoadedStrategy,
}

const RUNTIME_BINDINGS: &[StrategyRuntimeBinding] =
    &[eth_chainlink_taker::STRATEGY_RUNTIME_BINDING];

pub fn runtime_bindings() -> &'static [StrategyRuntimeBinding] {
    RUNTIME_BINDINGS
}

#[derive(Debug)]
pub enum BoltV3StrategyRegistrationError {
    UnsupportedArchetype {
        strategy_file: String,
        strategy_archetype: String,
    },
    MissingClient {
        strategy_file: String,
        client_id: String,
    },
    UnsupportedVenue {
        strategy_file: String,
        client_id: String,
        venue: String,
    },
    MissingExecutionBlock {
        strategy_file: String,
        client_id: String,
    },
    MissingProviderSecrets {
        strategy_file: String,
        client_id: String,
    },
    InvalidParameters {
        reason: String,
    },
    FeeProviderBuild {
        strategy_file: String,
        client_id: String,
        source: String,
    },
    AddStrategy {
        strategy_file: String,
        source: anyhow::Error,
    },
}

impl std::fmt::Display for BoltV3StrategyRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedArchetype {
                strategy_file,
                strategy_archetype,
            } => write!(
                f,
                "strategy `{strategy_file}` uses unsupported strategy_archetype `{strategy_archetype}`"
            ),
            Self::MissingClient {
                strategy_file,
                client_id,
            } => write!(
                f,
                "strategy `{strategy_file}` references missing client_id `{client_id}`"
            ),
            Self::UnsupportedVenue {
                strategy_file,
                client_id,
                venue,
            } => write!(
                f,
                "strategy `{strategy_file}` references client_id `{client_id}` with unsupported venue `{venue}`"
            ),
            Self::MissingExecutionBlock {
                strategy_file,
                client_id,
            } => write!(
                f,
                "strategy `{strategy_file}` references client_id `{client_id}`, but that client has no execution block"
            ),
            Self::MissingProviderSecrets {
                strategy_file,
                client_id,
            } => write!(
                f,
                "strategy `{strategy_file}` references client_id `{client_id}`, but provider secrets are missing"
            ),
            Self::InvalidParameters { reason } => write!(f, "{reason}"),
            Self::FeeProviderBuild {
                strategy_file,
                client_id,
                source,
            } => write!(
                f,
                "strategy `{strategy_file}` failed to build fee provider for client_id `{client_id}`: {source}"
            ),
            Self::AddStrategy {
                strategy_file,
                source,
            } => write!(
                f,
                "strategy `{strategy_file}` failed NT strategy registration: {source}"
            ),
        }
    }
}

impl std::error::Error for BoltV3StrategyRegistrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AddStrategy { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

pub fn register_bolt_v3_strategies(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<Vec<StrategyId>, BoltV3StrategyRegistrationError> {
    let mut registered = Vec::with_capacity(loaded.strategies.len());
    for strategy in &loaded.strategies {
        let binding = runtime_bindings()
            .iter()
            .find(|binding| binding.key == strategy.config.strategy_archetype.as_str())
            .ok_or_else(|| BoltV3StrategyRegistrationError::UnsupportedArchetype {
                strategy_file: strategy.relative_path.clone(),
                strategy_archetype: strategy.config.strategy_archetype.as_str().to_string(),
            })?;
        let context = StrategyRegistrationContext {
            loaded,
            resolved,
            strategy,
        };
        registered.push((binding.register)(node, context)?);
    }
    Ok(registered)
}
