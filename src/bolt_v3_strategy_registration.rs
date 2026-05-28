//! Generic strategy registration boundary for bolt-v3.
//!
//! This module iterates validated bolt-v3 strategy envelopes and delegates
//! concrete registration to an injected binding. Concrete strategy builders
//! stay outside this core boundary.

use crate::bolt_v3_config::{
    DECISION_REFERENCE_GATE_ROLE, LoadedBoltV3Config, LoadedStrategy, StrategyArchetypeKey,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3DecisionEvidenceWriter, BoltV3GateEvidenceIdentity, BoltV3ReadinessGateEvidenceSnapshot,
    BoltV3RuntimeReadinessSeed, validate_readiness_gate_evidence_snapshot,
};
use crate::bolt_v3_operator_artifacts::{EntryReadinessGateSession, read_file_bounded};
use crate::bolt_v3_secrets::ResolvedBoltV3Secrets;
use crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState;
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

const OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD: &str = "gate_session_path";
const OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD: &str = "expected_gate_session_sha256";
const OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_PATH_FIELD: &str = "strategy_input_evidence_path";
const OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_SHA256_FIELD: &str =
    "strategy_input_evidence_sha256";

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
    pub runtime_readiness_seed: Option<BoltV3RuntimeReadinessSeed>,
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
    runtime_seed: Option<BoltV3RuntimeReadinessSeed>,
}

#[derive(Deserialize)]
struct StrategyInputRuntimeSeedFile {
    strategy_instance_id: Option<String>,
    gate_session_hash: Option<String>,
    selected_market_key: Option<String>,
    gate_evidence: Option<BTreeMap<String, BoltV3GateEvidenceIdentity>>,
    realized_volatility: String,
    spot_price: String,
    price_to_beat_value: String,
    reference_quote_ts_event: u64,
    polymarket_condition_id: String,
    polymarket_market_slug: String,
    polymarket_question_id: String,
    up_instrument_id: String,
    down_instrument_id: String,
    polymarket_market_start_timestamp_ms: u64,
    polymarket_market_end_timestamp_ms: u64,
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
                runtime_readiness_seed: readiness_evidence
                    .as_ref()
                    .filter(|evidence| {
                        evidence.strategy_instance_id
                            == strategy.config.strategy_instance_id.as_str()
                    })
                    .and_then(|evidence| evidence.runtime_seed.clone()),
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
    let runtime_seed = load_runtime_readiness_seed(loaded, operator_evidence, &session, &snapshot)?;
    Ok(Some(LoadedStrategyReadinessEvidence {
        strategy_instance_id: session.strategy_instance_id,
        snapshot,
        runtime_seed,
    }))
}

fn load_runtime_readiness_seed(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &crate::bolt_v3_config::LiveCanaryOperatorEvidenceBlock,
    session: &EntryReadinessGateSession,
    snapshot: &BoltV3ReadinessGateEvidenceSnapshot,
) -> Result<Option<BoltV3RuntimeReadinessSeed>, BoltV3StrategyRegistrationError> {
    if !strategy_has_decision_reference(loaded, session.strategy_instance_id.as_str())? {
        return Ok(None);
    }
    let strategy_input_evidence_path = required_operator_evidence_field(
        OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_PATH_FIELD,
        Some(operator_evidence.strategy_input_evidence_path.as_str()),
    )?;
    let strategy_input_evidence_sha256 = required_operator_evidence_field(
        OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_SHA256_FIELD,
        Some(operator_evidence.strategy_input_evidence_sha256.as_str()),
    )?;
    let resolved_path = resolve_loaded_config_path(loaded, strategy_input_evidence_path);
    let bytes = read_file_bounded(
        &resolved_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(|source| BoltV3StrategyRegistrationError::Evidence {
        message: format!(
            "failed to read `[live_canary.operator_evidence].{}` at {}: {source}",
            OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_PATH_FIELD,
            resolved_path.display()
        ),
    })?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    if actual_sha256 != strategy_input_evidence_sha256 {
        return Err(BoltV3StrategyRegistrationError::Evidence {
            message: format!(
                "`[live_canary.operator_evidence].{}` does not match `{}`",
                OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_SHA256_FIELD,
                OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_PATH_FIELD
            ),
        });
    }
    let input: StrategyInputRuntimeSeedFile = serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3StrategyRegistrationError::Evidence {
            message: format!(
                "failed to parse `[live_canary.operator_evidence].{}` at {}: {source}",
                OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_PATH_FIELD,
                resolved_path.display()
            ),
        }
    })?;
    let strategy_instance_id = required_trimmed_seed_field(
        OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_PATH_FIELD,
        "strategy_instance_id",
        input.strategy_instance_id.as_deref(),
    )?;
    if strategy_instance_id != session.strategy_instance_id {
        return Err(strategy_seed_error(
            "strategy_input_evidence strategy_instance_id does not match gate session",
        ));
    }
    let gate_session_hash = required_trimmed_seed_field(
        OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_PATH_FIELD,
        "gate_session_hash",
        input.gate_session_hash.as_deref(),
    )?;
    if gate_session_hash != session.session_hash {
        return Err(strategy_seed_error(
            "strategy_input_evidence gate_session_hash does not match gate session",
        ));
    }
    let selected_market_key = required_trimmed_seed_field(
        OPERATOR_EVIDENCE_STRATEGY_INPUT_EVIDENCE_PATH_FIELD,
        "selected_market_key",
        input.selected_market_key.as_deref(),
    )?;
    if selected_market_key != snapshot.selected_market_key {
        return Err(strategy_seed_error(
            "strategy_input_evidence selected_market_key does not match gate session",
        ));
    }
    let reference_venue = source_owned_decision_reference_provider_id(
        loaded,
        snapshot,
        input.gate_evidence.as_ref(),
    )?;

    let price_to_beat_value =
        positive_seed_number("price_to_beat_value", input.price_to_beat_value.as_str())?;
    let reference_price = positive_seed_number("spot_price", input.spot_price.as_str())?;
    let realized_volatility =
        positive_seed_number("realized_volatility", input.realized_volatility.as_str())?;
    if input.reference_quote_ts_event == 0 {
        return Err(strategy_seed_error(
            "strategy_input_evidence reference_quote_ts_event is invalid",
        ));
    }
    if input.reference_quote_ts_event < input.polymarket_market_start_timestamp_ms {
        return Err(strategy_seed_error(
            "strategy_input_evidence reference_quote_ts_event precedes selected market start",
        ));
    }

    Ok(Some(BoltV3RuntimeReadinessSeed {
        strategy_instance_id: strategy_instance_id.to_string(),
        gate_session_hash: gate_session_hash.to_string(),
        selected_market_key: selected_market_key.to_string(),
        polymarket_condition_id: required_owned_seed_string(
            "polymarket_condition_id",
            input.polymarket_condition_id,
        )?,
        polymarket_market_slug: required_owned_seed_string(
            "polymarket_market_slug",
            input.polymarket_market_slug,
        )?,
        polymarket_question_id: required_owned_seed_string(
            "polymarket_question_id",
            input.polymarket_question_id,
        )?,
        up_instrument_id: required_owned_seed_string("up_instrument_id", input.up_instrument_id)?,
        down_instrument_id: required_owned_seed_string(
            "down_instrument_id",
            input.down_instrument_id,
        )?,
        market_start_timestamp_ms: input.polymarket_market_start_timestamp_ms,
        market_end_timestamp_ms: input.polymarket_market_end_timestamp_ms,
        price_to_beat_value,
        reference_venue,
        reference_price,
        reference_quote_ts_event: input.reference_quote_ts_event,
        realized_volatility,
    }))
}

fn strategy_has_decision_reference(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
) -> Result<bool, BoltV3StrategyRegistrationError> {
    let Some(strategy) = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
    else {
        return Err(strategy_seed_error(
            "gate session strategy_instance_id does not match loaded config",
        ));
    };
    Ok(strategy
        .config
        .target
        .as_table()
        .and_then(|target| target.get("gate_subscriptions"))
        .and_then(toml::Value::as_table)
        .and_then(|subscriptions| subscriptions.get(DECISION_REFERENCE_GATE_ROLE))
        .is_some())
}

fn source_owned_decision_reference_provider_id(
    loaded: &LoadedBoltV3Config,
    snapshot: &BoltV3ReadinessGateEvidenceSnapshot,
    input_gate_evidence: Option<&BTreeMap<String, BoltV3GateEvidenceIdentity>>,
) -> Result<String, BoltV3StrategyRegistrationError> {
    let input_identity = input_gate_evidence
        .and_then(|gate_evidence| gate_evidence.get(DECISION_REFERENCE_GATE_ROLE))
        .ok_or_else(|| {
            strategy_seed_error(
                "strategy_input_evidence decision_reference gate identity is missing",
            )
        })?;
    let session_identity = snapshot
        .gate_evidence
        .get(DECISION_REFERENCE_GATE_ROLE)
        .ok_or_else(|| {
            strategy_seed_error("gate session decision_reference gate identity is missing")
        })?;
    if input_identity != session_identity {
        return Err(strategy_seed_error(
            "strategy_input_evidence decision_reference gate identity does not match gate session",
        ));
    }
    if input_identity.selected_market_key != snapshot.selected_market_key {
        return Err(strategy_seed_error(
            "strategy_input_evidence decision_reference selected_market_key does not match gate session",
        ));
    }
    let provider_id = input_identity
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|provider_id| !provider_id.is_empty())
        .ok_or_else(|| {
            strategy_seed_error("strategy_input_evidence decision_reference provider_id is missing")
        })?;
    if !loaded
        .root
        .gate_providers
        .as_ref()
        .is_some_and(|gate_providers| gate_providers.contains_key(provider_id))
    {
        return Err(strategy_seed_error(
            "decision_reference provider_id does not match loaded gate providers",
        ));
    }
    Ok(provider_id.to_string())
}

fn required_trimmed_seed_field<'a>(
    path_field: &'static str,
    field: &'static str,
    value: Option<&'a str>,
) -> Result<&'a str, BoltV3StrategyRegistrationError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| BoltV3StrategyRegistrationError::Evidence {
            message: format!("`[live_canary.operator_evidence].{path_field}` missing `{field}`"),
        })
}

fn required_owned_seed_string(
    field: &'static str,
    value: String,
) -> Result<String, BoltV3StrategyRegistrationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(strategy_seed_error(format!(
            "strategy_input_evidence {field} is missing"
        )));
    }
    Ok(trimmed.to_string())
}

fn positive_seed_number(
    field: &'static str,
    value: &str,
) -> Result<f64, BoltV3StrategyRegistrationError> {
    let parsed = value.trim().parse::<f64>().map_err(|source| {
        BoltV3StrategyRegistrationError::Evidence {
            message: format!("strategy_input_evidence {field} is invalid: {source}"),
        }
    })?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return Err(strategy_seed_error(format!(
            "strategy_input_evidence {field} is invalid"
        )));
    }
    Ok(parsed)
}

fn strategy_seed_error(message: impl Into<String>) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::Evidence {
        message: message.into(),
    }
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
