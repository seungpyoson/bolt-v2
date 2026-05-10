use std::collections::BTreeSet;

use nautilus_live::node::LiveNode;
use toml::Value;

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, REFERENCE_STREAM_ID_PARAMETER},
    bolt_v3_reference_producer::BoltV3ReferenceActorPlan,
};

#[derive(Debug)]
pub struct BoltV3ReferenceActorRegistrationError {
    reason: String,
}

impl BoltV3ReferenceActorRegistrationError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for BoltV3ReferenceActorRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for BoltV3ReferenceActorRegistrationError {}

pub fn register_bolt_v3_reference_actors(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<Vec<String>, BoltV3ReferenceActorRegistrationError> {
    let stream_ids = selected_reference_stream_ids(loaded)?;
    let mut registered = Vec::with_capacity(stream_ids.len());

    for stream_id in stream_ids {
        let stream = loaded
            .root
            .reference_streams
            .get(stream_id.as_str())
            .ok_or_else(|| {
                BoltV3ReferenceActorRegistrationError::new(format!(
                    "selected reference stream `{stream_id}` does not match any [reference_streams.<id>] block"
                ))
            })?;
        let plan = BoltV3ReferenceActorPlan::from_stream(&loaded.root, stream_id.as_str(), stream)
            .map_err(|source| {
                BoltV3ReferenceActorRegistrationError::new(format!(
                    "reference stream `{stream_id}` failed to build ReferenceActor plan: {source}"
                ))
            })?;
        node.add_actor(plan.into_actor()).map_err(|source| {
            BoltV3ReferenceActorRegistrationError::new(format!(
                "reference stream `{stream_id}` failed to register ReferenceActor on NT LiveNode: {source}"
            ))
        })?;
        registered.push(stream_id);
    }

    Ok(registered)
}

fn selected_reference_stream_ids(
    loaded: &LoadedBoltV3Config,
) -> Result<Vec<String>, BoltV3ReferenceActorRegistrationError> {
    let mut stream_ids = BTreeSet::new();
    for strategy in &loaded.strategies {
        let Value::Table(parameters) = &strategy.config.parameters else {
            continue;
        };
        match parameters.get(REFERENCE_STREAM_ID_PARAMETER) {
            Some(Value::String(value)) if !value.trim().is_empty() => {
                stream_ids.insert(value.clone());
            }
            Some(Value::String(_)) => {
                return Err(BoltV3ReferenceActorRegistrationError::new(format!(
                    "strategy `{}` parameters.{REFERENCE_STREAM_ID_PARAMETER} must be non-empty",
                    strategy.relative_path
                )));
            }
            Some(value) => {
                return Err(BoltV3ReferenceActorRegistrationError::new(format!(
                    "strategy `{}` parameters.{REFERENCE_STREAM_ID_PARAMETER} must be a string, got {value:?}",
                    strategy.relative_path
                )));
            }
            None => {}
        }
    }

    Ok(stream_ids.into_iter().collect())
}
