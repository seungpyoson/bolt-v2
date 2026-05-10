use anyhow::{Result, bail};
use toml::Value;

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy, RuntimeMode},
    bolt_v3_decision_events::BoltV3DecisionEventCommonFields,
};

const DECISION_EVENT_SCHEMA_VERSION: u64 = 1;
const CONFIGURED_TARGET_ID_FIELD: &str = "configured_target_id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3DecisionEventIdentity {
    pub release_id: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
}

pub fn bolt_v3_decision_event_common_fields(
    loaded: &LoadedBoltV3Config,
    strategy: &LoadedStrategy,
    identity: &BoltV3DecisionEventIdentity,
    decision_trace_id: &str,
) -> Result<BoltV3DecisionEventCommonFields> {
    validate_identity(identity)?;
    if decision_trace_id.trim().is_empty() {
        bail!(
            "strategy `{}` decision_trace_id must be non-empty",
            strategy.relative_path
        );
    }

    let client = loaded
        .root
        .clients
        .get(&strategy.config.execution_client_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "strategy `{}` references missing execution_client_id `{}`",
                strategy.relative_path,
                strategy.config.execution_client_id
            )
        })?;

    Ok(BoltV3DecisionEventCommonFields {
        schema_version: DECISION_EVENT_SCHEMA_VERSION,
        decision_trace_id: decision_trace_id.to_string(),
        strategy_instance_id: strategy.config.strategy_instance_id.clone(),
        strategy_archetype: strategy.config.strategy_archetype.as_str().to_string(),
        trader_id: loaded.root.trader_id.clone(),
        client_id: strategy.config.execution_client_id.clone(),
        venue: client.venue.as_str().to_string(),
        runtime_mode: runtime_mode_as_str(loaded.root.runtime.mode).to_string(),
        release_id: identity.release_id.clone(),
        config_hash: identity.config_hash.clone(),
        nautilus_trader_revision: identity.nautilus_trader_revision.clone(),
        configured_target_id: configured_target_id(strategy)?.to_string(),
    })
}

fn validate_identity(identity: &BoltV3DecisionEventIdentity) -> Result<()> {
    if identity.release_id.trim().is_empty() {
        bail!("release_id must be non-empty");
    }
    if identity.config_hash.trim().is_empty() {
        bail!("config_hash must be non-empty");
    }
    if identity.nautilus_trader_revision.trim().is_empty() {
        bail!("nautilus_trader_revision must be non-empty");
    }
    Ok(())
}

fn configured_target_id(strategy: &LoadedStrategy) -> Result<&str> {
    let Value::Table(target) = &strategy.config.target else {
        bail!(
            "strategy `{}` target must be a table",
            strategy.relative_path
        );
    };
    match target.get(CONFIGURED_TARGET_ID_FIELD) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value),
        Some(Value::String(_)) => bail!(
            "strategy `{}` target.{CONFIGURED_TARGET_ID_FIELD} must be non-empty",
            strategy.relative_path
        ),
        Some(value) => bail!(
            "strategy `{}` target.{CONFIGURED_TARGET_ID_FIELD} must be a string, got {value:?}",
            strategy.relative_path
        ),
        None => bail!(
            "strategy `{}` requires target.{CONFIGURED_TARGET_ID_FIELD}",
            strategy.relative_path
        ),
    }
}

fn runtime_mode_as_str(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::Live => "live",
    }
}
