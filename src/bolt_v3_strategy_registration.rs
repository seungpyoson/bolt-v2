//! Generic strategy registration boundary for bolt-v3.
//!
//! This module iterates validated bolt-v3 strategy envelopes and delegates
//! concrete registration to an injected binding. Concrete strategy builders
//! stay outside this core boundary.

use crate::bolt_v3_config::{
    BoltV3RootConfig, LoadedBoltV3Config, LoadedStrategy, StrategyArchetypeKey,
};
use crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter;
use crate::bolt_v3_iv::{
    config::IvProfile,
    query::{IvQueryHandle, IvStrategyQueryHandle},
    runtime::{IvRuntimeEngine, runtime_derived_inputs_from_profile},
    store::{IvRetentionPolicy, IvStore},
};
use crate::bolt_v3_operator_health::BoltV3SettlementHealthTransitionEmitter;
use crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy;
use crate::bolt_v3_providers::resolve_fee_provider;
use crate::bolt_v3_secrets::ResolvedBoltV3Secrets;
use crate::bolt_v3_settlement_runtime::{
    BoltV3SettlementRecoveryConfig, BoltV3SettlementRuntimeSinkHandle,
};
use crate::bolt_v3_strategy_context::StrategyBuildContext;
use crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState;
use nautilus_live::node::LiveNode;
use nautilus_model::{
    identifiers::{ClientId, StrategyId, Venue},
    types::Currency,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime;

#[derive(Clone, Copy)]
pub struct StrategyRuntimeBinding {
    pub key: &'static str,
    pub strategy_kind: &'static str,
    pub capabilities: StrategyRuntimeCapabilities,
    pub register: for<'a> fn(
        &mut LiveNode,
        StrategyRegistrationContext<'a>,
    ) -> Result<StrategyId, BoltV3StrategyRegistrationError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StrategyRuntimeCapabilities {
    pub realized_volatility: bool,
    pub settlement: bool,
}

#[derive(Clone)]
pub struct BoltV3StrategyExecutionControls {
    pub submit_admission: Arc<BoltV3SubmitAdmissionState>,
    pub order_execution_policy: BoltV3OrderExecutionPolicy,
    pub settlement_runtime_sink: Option<BoltV3SettlementRuntimeSinkHandle>,
    pub settlement_recovery: Option<BoltV3SettlementRecoveryConfig>,
    pub settlement_health_transition_emitter: Option<BoltV3SettlementHealthTransitionEmitter>,
}

#[derive(Clone)]
pub struct StrategyRegistrationContext<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy: &'a LoadedStrategy,
    pub strategy_kind: &'static str,
    pub capabilities: StrategyRuntimeCapabilities,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    pub submit_admission: Arc<BoltV3SubmitAdmissionState>,
    pub iv_query_handles: Arc<BoltV3IvQueryHandleRegistry>,
    pub order_execution_policy: BoltV3OrderExecutionPolicy,
    realized_volatility_runtime: Option<Arc<Mutex<RealizedVolSurfaceRuntime>>>,
    settlement: Option<StrategyRegistrationSettlementCapability>,
}

#[derive(Clone)]
enum StrategyRegistrationSettlementCapability {
    Resolved(StrategyRegistrationSettlementResources),
    Invalid(StrategyRegistrationSettlementIdentityError),
}

#[derive(Clone)]
struct StrategyRegistrationSettlementResources {
    execution_venue: Venue,
    settlement_account_id: String,
    settlement_currency: Currency,
    runtime_sink: Option<BoltV3SettlementRuntimeSinkHandle>,
    recovery: Option<BoltV3SettlementRecoveryConfig>,
    health_transition_emitter: Option<BoltV3SettlementHealthTransitionEmitter>,
}

#[derive(Clone)]
enum StrategyRegistrationSettlementIdentityError {
    ExecutionVenue { execution_client_id: String },
    AccountId { execution_client_id: String },
    Currency { settlement_account_id: String },
}

impl StrategyRegistrationSettlementIdentityError {
    fn message(&self) -> String {
        match self {
            Self::ExecutionVenue {
                execution_client_id,
            } => format!(
                "execution_client_id `{execution_client_id}` is not present in loaded clients for execution-venue resolution"
            ),
            Self::AccountId {
                execution_client_id,
            } => format!(
                "settlement capability requires execution account id for execution_client_id `{execution_client_id}`"
            ),
            Self::Currency {
                settlement_account_id,
            } => format!(
                "settlement capability requires settlement currency for execution account `{settlement_account_id}`"
            ),
        }
    }
}

impl<'a> StrategyRegistrationContext<'a> {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        loaded: &'a LoadedBoltV3Config,
        strategy: &'a LoadedStrategy,
        strategy_kind: &'static str,
        capabilities: StrategyRuntimeCapabilities,
        resolved: &'a ResolvedBoltV3Secrets,
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        iv_query_handles: Arc<BoltV3IvQueryHandleRegistry>,
        realized_volatility_runtime: Arc<Mutex<RealizedVolSurfaceRuntime>>,
        execution_controls: BoltV3StrategyExecutionControls,
    ) -> Self {
        let BoltV3StrategyExecutionControls {
            submit_admission,
            order_execution_policy,
            settlement_runtime_sink,
            settlement_recovery,
            settlement_health_transition_emitter,
        } = execution_controls;
        let realized_volatility_runtime = capabilities
            .realized_volatility
            .then_some(realized_volatility_runtime);
        let settlement = capabilities.settlement.then(|| {
            resolve_settlement_capability(
                loaded,
                strategy,
                settlement_runtime_sink,
                settlement_recovery,
                settlement_health_transition_emitter,
            )
        });

        Self {
            loaded,
            strategy,
            strategy_kind,
            capabilities,
            resolved,
            decision_evidence,
            submit_admission,
            iv_query_handles,
            order_execution_policy,
            realized_volatility_runtime,
            settlement,
        }
    }
}

fn resolve_settlement_capability(
    loaded: &LoadedBoltV3Config,
    strategy: &LoadedStrategy,
    runtime_sink: Option<BoltV3SettlementRuntimeSinkHandle>,
    recovery: Option<BoltV3SettlementRecoveryConfig>,
    health_transition_emitter: Option<BoltV3SettlementHealthTransitionEmitter>,
) -> StrategyRegistrationSettlementCapability {
    let execution_client_id = strategy.config.execution_client_id.as_str();
    let Some(execution_venue) = venue_for_client(&loaded.root, execution_client_id) else {
        return StrategyRegistrationSettlementCapability::Invalid(
            StrategyRegistrationSettlementIdentityError::ExecutionVenue {
                execution_client_id: execution_client_id.to_string(),
            },
        );
    };
    let Some(settlement_account_id) = execution_account_id(&loaded.root, execution_client_id)
    else {
        return StrategyRegistrationSettlementCapability::Invalid(
            StrategyRegistrationSettlementIdentityError::AccountId {
                execution_client_id: execution_client_id.to_string(),
            },
        );
    };
    let Some(settlement_currency) = settlement_currency_for_execution_account(
        &loaded.root,
        execution_venue,
        settlement_account_id,
    ) else {
        return StrategyRegistrationSettlementCapability::Invalid(
            StrategyRegistrationSettlementIdentityError::Currency {
                settlement_account_id: settlement_account_id.to_string(),
            },
        );
    };
    StrategyRegistrationSettlementCapability::Resolved(StrategyRegistrationSettlementResources {
        execution_venue,
        settlement_account_id: settlement_account_id.to_string(),
        settlement_currency,
        runtime_sink,
        recovery,
        health_transition_emitter,
    })
}

pub fn assemble_strategy_build_context(
    context: &StrategyRegistrationContext<'_>,
) -> Result<StrategyBuildContext, BoltV3StrategyRegistrationError> {
    let execution_client_id = context.strategy.config.execution_client_id.as_str();
    let settlement = settlement_resources_for_context(context)?;
    let execution_venue = match settlement {
        Some(settlement) => settlement.execution_venue,
        None => execution_venue_for_context(context)?,
    };
    let fee_provider = resolve_fee_provider(context.loaded, execution_client_id, context.resolved)
        .map_err(|error| binding_message(context, error.to_string()))?;
    let mut build_context = StrategyBuildContext::new(
        fee_provider,
        context.decision_evidence.clone(),
        context.submit_admission.clone(),
        context.order_execution_policy,
        execution_venue,
    );
    if let Some(realized_volatility_runtime) = &context.realized_volatility_runtime {
        build_context =
            build_context.with_realized_volatility_runtime(realized_volatility_runtime.clone());
    }
    if let Some(settlement) = settlement {
        build_context = build_context
            .with_settlement_runtime_sink(settlement.runtime_sink.clone())
            .with_settlement_recovery(settlement.recovery.clone())
            .with_settlement_account_id(Some(settlement.settlement_account_id.clone()))
            .with_settlement_currency(Some(settlement.settlement_currency))
            .with_settlement_health_transition_emitter(
                settlement.health_transition_emitter.clone(),
            );
    }
    Ok(build_context)
}

fn execution_venue_for_context(
    context: &StrategyRegistrationContext<'_>,
) -> Result<Venue, BoltV3StrategyRegistrationError> {
    let execution_client_id = context.strategy.config.execution_client_id.as_str();
    venue_for_client(&context.loaded.root, execution_client_id).ok_or_else(|| {
        binding_message(
            context,
            format!(
                "execution_client_id `{execution_client_id}` is not present in loaded clients for execution-venue resolution"
            ),
        )
    })
}

fn settlement_resources_for_context<'a>(
    context: &'a StrategyRegistrationContext<'_>,
) -> Result<Option<&'a StrategyRegistrationSettlementResources>, BoltV3StrategyRegistrationError> {
    match &context.settlement {
        None => Ok(None),
        Some(StrategyRegistrationSettlementCapability::Resolved(resources)) => Ok(Some(resources)),
        Some(StrategyRegistrationSettlementCapability::Invalid(error)) => {
            Err(binding_message(context, error.message()))
        }
    }
}

/// Neutral client-table venue lookup for execution and data client ids alike, so
/// archetypes never touch `root.clients` directly.
pub(crate) fn venue_for_client(root: &BoltV3RootConfig, client_id: &str) -> Option<Venue> {
    root.clients.get(client_id).map(|client| client.venue)
}

pub(crate) fn execution_account_id<'a>(
    root: &'a BoltV3RootConfig,
    execution_client_id: &str,
) -> Option<&'a str> {
    root.clients
        .get(execution_client_id)?
        .execution
        .as_ref()?
        .as_table()?
        .get(stringify!(account_id))?
        .as_str()
}

pub(crate) fn settlement_currency_for_execution_account(
    root: &BoltV3RootConfig,
    execution_venue: Venue,
    account_id: &str,
) -> Option<Currency> {
    root.risk
        .capital_pools
        .as_ref()?
        .iter()
        .find(|pool| {
            pool.venue_id == execution_venue.as_str() && pool.account_id.to_string() == account_id
        })
        .and_then(|pool| settlement_currency_from_config_code(pool.collateral_currency.as_str()))
}

pub(crate) fn settlement_currency_from_config_code(configured: &str) -> Option<Currency> {
    let pusd = Currency::pUSD();
    if configured.eq_ignore_ascii_case(pusd.code.as_str()) {
        Some(pusd)
    } else {
        configured.parse().ok()
    }
}

fn binding_message(
    context: &StrategyRegistrationContext<'_>,
    message: String,
) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::Binding {
        strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
        strategy_archetype: context
            .strategy
            .config
            .strategy_archetype
            .as_str()
            .to_string(),
        message,
    }
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

#[derive(Clone, Debug)]
pub struct BoltV3IvQueryHandleRegistry {
    handles: BTreeMap<(String, String), IvStrategyQueryHandle>,
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

    pub fn handle(&self, strategy_id: &str, profile_id: &str) -> Option<&IvStrategyQueryHandle> {
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
    RealizedVolatilityRuntime {
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
            Self::RealizedVolatilityRuntime { message } => {
                write!(
                    f,
                    "bolt-v3 realized-volatility runtime setup failed: {message}"
                )
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
        let mut profile_store = store.clone();
        profile_store.set_input_bounds(profile.input_bounds.clone());
        for authorization in profile.strategy_authorizations() {
            let current_generations = profile
                .sources
                .iter()
                .map(|source| (source.source_id.clone(), source.subscription_generation))
                .collect();
            let key = (
                authorization.strategy_id.clone(),
                profile.profile_id.clone(),
            );
            if handles
                .insert(
                    key.clone(),
                    IvStrategyQueryHandle::new(
                        IvQueryHandle::new(
                            &profile.profile_id,
                            authorization,
                            profile_store.clone(),
                        )
                        .with_projection_policies(profile.projection_policies.clone())
                        .with_interpolation_policies(profile.interpolation_policies.clone())
                        .with_fallback_policies(profile.fallback_policies.clone())
                        .with_quorum_policies(profile.quorum_policies.clone())
                        .with_helper_policies(profile.helper_policies.clone())
                        .with_derived_input_policies(profile.derived_input_policies.clone())
                        .with_derived_inputs(runtime_derived_inputs_from_profile(profile))
                        .with_retention_policy(retention_policy_from_profile(profile))
                        .with_current_subscription_generations(current_generations),
                    ),
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

pub fn build_iv_query_handle_registry_for_runtime(
    root: &BoltV3RootConfig,
    runtime: &IvRuntimeEngine,
) -> Result<BoltV3IvQueryHandleRegistry, BoltV3StrategyRegistrationError> {
    let Some(iv) = &root.iv else {
        return Ok(BoltV3IvQueryHandleRegistry::empty());
    };

    let mut handles = BTreeMap::new();
    for profile in &iv.profiles {
        let state = runtime
            .state_for_profile(&profile.profile_id)
            .ok_or_else(
                || BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                    message: format!(
                        "missing IV runtime state for profile {}",
                        profile.profile_id
                    ),
                },
            )?;
        for authorization in profile.strategy_authorizations() {
            let key = (
                authorization.strategy_id.clone(),
                profile.profile_id.clone(),
            );
            if handles
                .insert(
                    key.clone(),
                    IvStrategyQueryHandle::new(
                        IvQueryHandle::from_state(
                            &profile.profile_id,
                            authorization,
                            state.clone(),
                        )
                        .with_retention_policy(retention_policy_from_profile(profile)),
                    ),
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

fn retention_policy_from_profile(profile: &IvProfile) -> IvRetentionPolicy {
    IvRetentionPolicy {
        max_raw_events: profile.max_raw_events,
        max_indexed_points: profile.max_indexed_points,
        max_smiles: profile.max_smiles,
        max_surfaces: profile.max_surfaces,
        max_derived_points: profile.max_derived_points,
        max_source_health_events: profile.max_source_health_events,
    }
}

pub fn build_iv_query_handle_registry(
    loaded: &LoadedBoltV3Config,
    store: IvStore,
) -> Result<BoltV3IvQueryHandleRegistry, BoltV3StrategyRegistrationError> {
    validate_iv_strategy_references(loaded)?;
    let registry = build_iv_query_handle_registry_for_root(&loaded.root, store)?;

    Ok(registry)
}

pub fn validate_iv_strategy_references(
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3StrategyRegistrationError> {
    let Some(iv) = &loaded.root.iv else {
        return Ok(());
    };
    let configured_strategy_instance_ids = loaded
        .strategies
        .iter()
        .map(|strategy| strategy.config.strategy_instance_id.as_str())
        .collect::<BTreeSet<_>>();
    for profile in &iv.profiles {
        for authorization in &profile.strategy_authorizations {
            if !configured_strategy_instance_ids.contains(authorization.strategy_id.as_str()) {
                return Err(BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                    message: format!(
                        "iv profile {} references unknown strategy {}",
                        profile.profile_id, authorization.strategy_id
                    ),
                });
            }
        }
    }

    Ok(())
}

pub fn register_bolt_v3_strategies_on_node_with_bindings(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    bindings: &[StrategyRuntimeBinding],
    execution_controls: BoltV3StrategyExecutionControls,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
) -> Result<BoltV3StrategyRegistrationSummary, BoltV3StrategyRegistrationError> {
    if loaded.strategies.is_empty() {
        return Ok(BoltV3StrategyRegistrationSummary::empty());
    }
    if loaded.root.iv.is_some() {
        return Err(BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
            message: "IV-enabled configs must use runtime-backed strategy registration".to_string(),
        });
    }
    validate_iv_strategy_references(loaded)?;
    let iv_query_handles = Arc::new(build_iv_query_handle_registry(loaded, IvStore::empty())?);
    register_bolt_v3_strategies_on_node_with_handle_registry(
        node,
        loaded,
        resolved,
        bindings,
        execution_controls,
        decision_evidence,
        iv_query_handles,
    )
}

pub fn register_bolt_v3_strategies_on_node_with_iv_runtime_bindings(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    bindings: &[StrategyRuntimeBinding],
    execution_controls: BoltV3StrategyExecutionControls,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    iv_runtime: &IvRuntimeEngine,
) -> Result<BoltV3StrategyRegistrationSummary, BoltV3StrategyRegistrationError> {
    if loaded.strategies.is_empty() {
        return Ok(BoltV3StrategyRegistrationSummary::empty());
    }
    validate_iv_strategy_references(loaded)?;
    let iv_query_handles = Arc::new(build_iv_query_handle_registry_for_runtime(
        &loaded.root,
        iv_runtime,
    )?);
    register_bolt_v3_strategies_on_node_with_handle_registry(
        node,
        loaded,
        resolved,
        bindings,
        execution_controls,
        decision_evidence,
        iv_query_handles,
    )
}

fn register_bolt_v3_strategies_on_node_with_handle_registry(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    bindings: &[StrategyRuntimeBinding],
    execution_controls: BoltV3StrategyExecutionControls,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    iv_query_handles: Arc<BoltV3IvQueryHandleRegistry>,
) -> Result<BoltV3StrategyRegistrationSummary, BoltV3StrategyRegistrationError> {
    let mut summary = BoltV3StrategyRegistrationSummary::empty();
    validate_realized_volatility_runtime_source_clients(loaded)?;
    validate_realized_volatility_node_transport_membership(node, loaded)?;
    let realized_volatility_runtime = Arc::new(Mutex::new(
        RealizedVolSurfaceRuntime::from_loaded_config(loaded).map_err(|error| {
            BoltV3StrategyRegistrationError::RealizedVolatilityRuntime { message: error }
        })?,
    ));

    let prepared = loaded
        .strategies
        .iter()
        .map(|strategy| {
            let binding = bindings
                .iter()
                .find(|binding| binding.key == strategy.config.strategy_archetype.as_str())
                .ok_or_else(|| BoltV3StrategyRegistrationError::UnsupportedStrategy {
                    strategy_archetype: strategy.config.strategy_archetype.as_str().to_string(),
                })?;
            let context = StrategyRegistrationContext::new(
                loaded,
                strategy,
                binding.strategy_kind,
                binding.capabilities,
                resolved,
                decision_evidence.clone(),
                iv_query_handles.clone(),
                realized_volatility_runtime.clone(),
                execution_controls.clone(),
            );
            settlement_resources_for_context(&context)?;
            Ok((binding, context))
        })
        .collect::<Result<Vec<_>, BoltV3StrategyRegistrationError>>()?;

    for (binding, context) in prepared {
        let strategy = context.strategy;
        let registered_strategy_id = (binding.register)(node, context)?;
        summary.registered.push(BoltV3RegisteredStrategy {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            strategy_archetype: strategy.config.strategy_archetype.clone(),
            registered_strategy_id: registered_strategy_id.to_string(),
        });
    }

    Ok(summary)
}

fn validate_realized_volatility_runtime_source_clients(
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3StrategyRegistrationError> {
    let errors = crate::bolt_v3_validate::validate_realized_volatility_source_clients(&loaded.root);
    if errors.is_empty() {
        return Ok(());
    }
    Err(BoltV3StrategyRegistrationError::RealizedVolatilityRuntime {
        message: format!(
            "realized-volatility source client validation failed: {}",
            errors.join("; ")
        ),
    })
}

fn validate_realized_volatility_node_transport_membership(
    node: &LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3StrategyRegistrationError> {
    let Some(realized_volatility_surfaces) = loaded.root.realized_volatility_surfaces.as_ref()
    else {
        return Ok(());
    };

    let registered = node
        .kernel()
        .data_engine
        .borrow()
        .registered_clients()
        .into_iter()
        .collect::<BTreeSet<ClientId>>();
    let mut missing = BTreeSet::new();
    for (surface_id, surface) in realized_volatility_surfaces {
        for source in surface.sources.iter().filter(|source| source.enabled) {
            let client_id = ClientId::from(source.data_client_id.as_str());
            if !registered.contains(&client_id) {
                missing.insert(format!(
                    "realized_volatility_surfaces.{surface_id}.sources.{}.data_client_id `{}`",
                    source.source_id, source.data_client_id
                ));
            }
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(BoltV3StrategyRegistrationError::RealizedVolatilityRuntime {
        message: format!(
            "realized-volatility source client(s) not registered on this node's transport \
             (built without RV retention?): {}",
            missing.into_iter().collect::<Vec<_>>().join("; ")
        ),
    })
}
