//! Generic strategy registration boundary for bolt-v3.
//!
//! This module iterates validated bolt-v3 strategy envelopes and delegates
//! concrete registration to an injected binding. Concrete strategy builders
//! stay outside this core boundary.

use crate::bolt_v3_config::{
    BoltV3RootConfig, LoadedBoltV3Config, LoadedStrategy, StrategyArchetypeKey,
};
use crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter;
use crate::bolt_v3_iv::{query::IvQueryHandle, store::IvStore};
use crate::bolt_v3_secrets::ResolvedBoltV3Secrets;
use crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState;
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct StrategyRuntimeBinding {
    pub key: &'static str,
    pub strategy_kind: fn() -> &'static str,
    pub register: for<'a> fn(
        &mut LiveNode,
        StrategyRegistrationContext<'a>,
    ) -> Result<StrategyId, BoltV3StrategyRegistrationError>,
}

#[derive(Clone)]
pub struct StrategyRegistrationContext<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy: &'a LoadedStrategy,
    pub strategy_kind: &'static str,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    pub submit_admission: Arc<BoltV3SubmitAdmissionState>,
    pub iv_query_handles: Arc<BoltV3IvQueryHandleRegistry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3RegisteredStrategy {
    pub strategy_instance_id: String,
    pub strategy_archetype: StrategyArchetypeKey,
    pub registered_strategy_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3StrategyRegistrationSummary {
    pub registered: Vec<BoltV3RegisteredStrategy>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoltV3IvQueryHandleRegistry {
    handles: BTreeMap<(String, String), IvQueryHandle>,
}

impl BoltV3IvQueryHandleRegistry {
    pub fn empty() -> Self {
        Self {
            handles: BTreeMap::new(),
        }
    }

    pub fn handle_count(&self) -> usize {
        self.handles.len()
    }

    pub fn handle(&self, strategy_id: &str, profile_id: &str) -> Option<&IvQueryHandle> {
        self.handles
            .get(&(strategy_id.to_string(), profile_id.to_string()))
    }
}

impl BoltV3StrategyRegistrationSummary {
    fn empty() -> Self {
        Self {
            registered: Vec::new(),
        }
    }
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
    Evidence {
        message: String,
    },
    IvQueryHandleRegistration {
        message: String,
    },
}

impl std::fmt::Display for BoltV3StrategyRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedStrategy { strategy_archetype } => {
                write!(
                    f,
                    "unsupported bolt-v3 strategy archetype `{strategy_archetype}`"
                )
            }
            Self::Binding {
                strategy_instance_id,
                strategy_archetype,
                message,
            } => write!(
                f,
                "strategies.{strategy_instance_id} ({strategy_archetype}) registration failed: {message}"
            ),
            Self::Evidence { message } => {
                write!(f, "bolt-v3 decision evidence setup failed: {message}")
            }
            Self::IvQueryHandleRegistration { message } => {
                write!(f, "bolt-v3 IV query handle registration failed: {message}")
            }
        }
    }
}

impl std::error::Error for BoltV3StrategyRegistrationError {}

pub fn build_iv_query_handle_registry_for_root(
    root: &BoltV3RootConfig,
    store: IvStore,
) -> Result<BoltV3IvQueryHandleRegistry, BoltV3StrategyRegistrationError> {
    let Some(iv) = &root.iv else {
        return Ok(BoltV3IvQueryHandleRegistry::empty());
    };

    let mut handles = BTreeMap::new();
    for profile in &iv.profiles {
        for authorization in profile.strategy_authorizations() {
            let key = (
                authorization.strategy_id.clone(),
                profile.profile_id.clone(),
            );
            if handles
                .insert(
                    key.clone(),
                    IvQueryHandle::new(&profile.profile_id, authorization, store.clone()),
                )
                .is_some()
            {
                return Err(BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                    message: format!(
                        "duplicate IV query handle for strategy {} profile {}",
                        key.0, key.1
                    ),
                });
            }
        }
    }

    Ok(BoltV3IvQueryHandleRegistry { handles })
}

pub fn build_iv_query_handle_registry(
    loaded: &LoadedBoltV3Config,
    store: IvStore,
) -> Result<BoltV3IvQueryHandleRegistry, BoltV3StrategyRegistrationError> {
    let registry = build_iv_query_handle_registry_for_root(&loaded.root, store)?;
    if registry.handles.is_empty() {
        return Ok(registry);
    }

    let configured_strategy_ids = loaded
        .strategies
        .iter()
        .map(|strategy| strategy.config.strategy_instance_id.as_str())
        .collect::<BTreeSet<_>>();
    for (strategy_id, profile_id) in registry.handles.keys() {
        if !configured_strategy_ids.contains(strategy_id.as_str()) {
            return Err(BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                message: format!(
                    "iv profile {profile_id} references unknown strategy {strategy_id}"
                ),
            });
        }
    }

    Ok(registry)
}

pub fn register_bolt_v3_strategies_on_node_with_bindings(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    bindings: &[StrategyRuntimeBinding],
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
) -> Result<BoltV3StrategyRegistrationSummary, BoltV3StrategyRegistrationError> {
    let mut summary = BoltV3StrategyRegistrationSummary::empty();
    if loaded.strategies.is_empty() {
        return Ok(summary);
    }
    let iv_query_handles = Arc::new(build_iv_query_handle_registry(loaded, IvStore::default())?);

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
                strategy_kind: (binding.strategy_kind)(),
                resolved,
                decision_evidence: decision_evidence.clone(),
                submit_admission: submit_admission.clone(),
                iv_query_handles: iv_query_handles.clone(),
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
