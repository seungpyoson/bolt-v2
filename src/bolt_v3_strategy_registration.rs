//! Generic strategy registration boundary for bolt-v3.
//!
//! This module iterates validated bolt-v3 strategy envelopes and delegates
//! concrete registration to an injected binding. Concrete strategy builders
//! stay outside this core boundary.

use crate::bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy, StrategyArchetypeKey};
use crate::bolt_v3_decision_evidence::{
    BoltV3DecisionEvidenceWriter, BoltV3ReadinessGateEvidenceSnapshot,
    validate_readiness_gate_evidence_snapshot,
};
use crate::bolt_v3_operator_artifacts::{EntryReadinessGateSession, read_file_bounded};
use crate::bolt_v3_secrets::ResolvedBoltV3Secrets;
use crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState;
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD: &str = "gate_session_path";
const OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD: &str = "expected_gate_session_sha256";

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
    pub readiness_evidence: Option<BoltV3ReadinessGateEvidenceSnapshot>,
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

#[derive(Clone)]
struct LoadedStrategyReadinessEvidence {
    strategy_instance_id: String,
    snapshot: BoltV3ReadinessGateEvidenceSnapshot,
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
        }
    }
}

impl std::error::Error for BoltV3StrategyRegistrationError {}

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
    let readiness_evidence = load_strategy_registration_readiness_evidence(loaded)?;

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
                readiness_evidence: readiness_evidence
                    .as_ref()
                    .filter(|evidence| {
                        evidence.strategy_instance_id
                            == strategy.config.strategy_instance_id.as_str()
                    })
                    .map(|evidence| evidence.snapshot.clone()),
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

fn load_strategy_registration_readiness_evidence(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LoadedStrategyReadinessEvidence>, BoltV3StrategyRegistrationError> {
    let Some(live_canary) = loaded.root.live_canary.as_ref() else {
        return Ok(None);
    };
    let Some(operator_evidence) = live_canary.operator_evidence.as_ref() else {
        return Ok(None);
    };
    let gate_session_path = required_operator_evidence_field(
        OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD,
        operator_evidence.gate_session_path.as_deref(),
    )?;
    let expected_gate_session_sha256 = required_operator_evidence_field(
        OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD,
        operator_evidence.expected_gate_session_sha256.as_deref(),
    )?;
    let resolved_path = resolve_loaded_config_path(loaded, gate_session_path);
    let bytes = read_file_bounded(
        &resolved_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(|source| BoltV3StrategyRegistrationError::Evidence {
        message: format!(
            "failed to read `[live_canary.operator_evidence].{}` at {}: {source}",
            OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD,
            resolved_path.display()
        ),
    })?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    if actual_sha256 != expected_gate_session_sha256 {
        return Err(BoltV3StrategyRegistrationError::Evidence {
            message: format!(
                "`[live_canary.operator_evidence].{}` does not match `{}`",
                OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD,
                OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD
            ),
        });
    }
    let session: EntryReadinessGateSession = serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3StrategyRegistrationError::Evidence {
            message: format!(
                "failed to parse `[live_canary.operator_evidence].{}` at {}: {source}",
                OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD,
                resolved_path.display()
            ),
        }
    })?;
    validate_registration_gate_session(loaded, &session)
        .map_err(|message| BoltV3StrategyRegistrationError::Evidence { message })?;
    let snapshot = BoltV3ReadinessGateEvidenceSnapshot::from_entry_readiness_gate_session(&session);
    Ok(Some(LoadedStrategyReadinessEvidence {
        strategy_instance_id: session.strategy_instance_id,
        snapshot,
    }))
}

fn required_operator_evidence_field<'a>(
    field: &'static str,
    value: Option<&'a str>,
) -> Result<&'a str, BoltV3StrategyRegistrationError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| BoltV3StrategyRegistrationError::Evidence {
            message: format!("`[live_canary.operator_evidence].{field}` is required"),
        })
}

fn validate_registration_gate_session(
    loaded: &LoadedBoltV3Config,
    session: &EntryReadinessGateSession,
) -> Result<(), String> {
    let snapshot = BoltV3ReadinessGateEvidenceSnapshot::from_entry_readiness_gate_session(session);
    validate_readiness_gate_evidence_snapshot(&snapshot).map_err(|error| error.to_string())?;
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == session.strategy_instance_id)
        .ok_or_else(|| {
            "gate session strategy_instance_id does not match loaded config".to_string()
        })?;
    let target = strategy
        .config
        .target
        .as_table()
        .ok_or_else(|| "gate session strategy target is not a table".to_string())?;
    let configured_target_id = target
        .get("configured_target_id")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| {
            "gate session strategy target configured_target_id is missing".to_string()
        })?;
    if configured_target_id != session.configured_target_id
        || configured_target_id != session.selected_market.configured_target_id
    {
        return Err(
            "gate session configured_target_id does not match loaded strategy target".to_string(),
        );
    }
    Ok(())
}

fn resolve_loaded_config_path(loaded: &LoadedBoltV3Config, configured_path: &str) -> PathBuf {
    let path = Path::new(configured_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        match loaded.root_path.parent() {
            Some(parent) => parent.join(path),
            None => path.to_path_buf(),
        }
    }
}
