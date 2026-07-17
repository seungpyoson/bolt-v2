//! Generic strategy registration boundary for bolt-v3.
//!
//! This module iterates validated bolt-v3 strategy envelopes and delegates
//! concrete registration to an injected binding. Concrete strategy builders
//! stay outside this core boundary.

use crate::bolt_v3_config::{
    BoltV3RootConfig, LoadedBoltV3Config, LoadedStrategy, StrategyArchetypeKey,
};
use crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter;
use crate::bolt_v3_economics_runtime::{
    AuthoritativeEconomicsInputStore, ConfiguredEconomicsAdmissionSource,
    ConfiguredEconomicsSourcePolicy,
};
use crate::bolt_v3_iv::{
    config::IvProfile,
    query::{IvQueryHandle, IvStrategyQueryHandle},
    runtime::{IvRuntimeEngine, runtime_derived_inputs_from_profile},
    store::{IvRetentionPolicy, IvStore},
};
use crate::bolt_v3_numeric::{MILLIS_PER_SECOND_U64, NANOS_PER_MILLI_U64};
use crate::bolt_v3_operator_health::BoltV3SettlementHealthTransitionEmitter;
use crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy;
use crate::bolt_v3_order_execution::{BoltV3CarryPlan, BoltV3OrderRoutingHandle};
use crate::bolt_v3_providers::binding_for_provider_key;
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
    pub strategy_kind: fn() -> &'static str,
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
    pub economics_inputs: AuthoritativeEconomicsInputStore,
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
    pub realized_volatility_runtime: Arc<Mutex<RealizedVolSurfaceRuntime>>,
    pub settlement_runtime_sink: Option<BoltV3SettlementRuntimeSinkHandle>,
    pub settlement_recovery: Option<BoltV3SettlementRecoveryConfig>,
    pub settlement_health_transition_emitter: Option<BoltV3SettlementHealthTransitionEmitter>,
    pub economics_inputs: AuthoritativeEconomicsInputStore,
}

pub fn assemble_strategy_build_context(
    context: &StrategyRegistrationContext<'_>,
) -> Result<StrategyBuildContext, BoltV3StrategyRegistrationError> {
    let execution_client_id = context.strategy.config.execution_client_id.as_str();
    let execution_venue = venue_for_client(&context.loaded.root, execution_client_id)
        .ok_or_else(|| {
            binding_message(
                context,
                format!(
                    "execution_client_id `{execution_client_id}` is not present in loaded clients for execution-venue resolution"
                ),
            )
        })?;
    let mut build_context = StrategyBuildContext::new(
        context.decision_evidence.clone(),
        context.submit_admission.clone(),
        context.order_execution_policy,
        execution_venue,
    )
    .with_order_routing(build_order_routing_handle(context, execution_client_id)?);
    if context.capabilities.realized_volatility {
        build_context = build_context
            .with_realized_volatility_runtime(context.realized_volatility_runtime.clone());
    }
    if context.capabilities.settlement {
        let settlement_account_id =
            execution_account_id(&context.loaded.root, execution_client_id).map(str::to_string);
        let settlement_currency = settlement_account_id.as_deref().and_then(|account_id| {
            settlement_currency_for_execution_account(
                &context.loaded.root,
                execution_venue,
                account_id,
            )
        });
        build_context = build_context
            .with_settlement_runtime_sink(context.settlement_runtime_sink.clone())
            .with_settlement_recovery(context.settlement_recovery.clone())
            .with_settlement_account_id(settlement_account_id)
            .with_settlement_currency(settlement_currency)
            .with_settlement_health_transition_emitter(
                context.settlement_health_transition_emitter.clone(),
            );
    }
    Ok(build_context)
}

fn build_order_routing_handle(
    context: &StrategyRegistrationContext<'_>,
    execution_client_id: &str,
) -> Result<BoltV3OrderRoutingHandle, BoltV3StrategyRegistrationError> {
    let client = context
        .loaded
        .root
        .clients
        .get(execution_client_id)
        .ok_or_else(|| binding_message(context, "execution client is missing".to_string()))?;
    let execution = client.execution.as_ref().ok_or_else(|| {
        binding_message(
            context,
            "execution client has no execution block".to_string(),
        )
    })?;
    let binding = binding_for_provider_key(client.venue.as_str()).ok_or_else(|| {
        binding_message(
            context,
            "execution client has no provider registry binding".to_string(),
        )
    })?;
    let load_economics = binding.execution_economics.ok_or_else(|| {
        binding_message(
            context,
            "execution client has no economics registry binding".to_string(),
        )
    })?;
    let economics = load_economics(execution).map_err(|error| {
        binding_message(
            context,
            format!("execution economics configuration is invalid: {error}"),
        )
    })?;
    let quote_validity_ns = economics
        .quote_validity_ms
        .checked_mul(NANOS_PER_MILLI_U64)
        .ok_or_else(|| {
            binding_message(
                context,
                "execution economics quote validity overflows nanoseconds".to_string(),
            )
        })?;
    let quote_refresh_ns = economics
        .quote_refresh_secs
        .checked_mul(MILLIS_PER_SECOND_U64)
        .and_then(|value| value.checked_mul(NANOS_PER_MILLI_U64))
        .ok_or_else(|| {
            binding_message(
                context,
                "execution economics quote refresh overflows nanoseconds".to_string(),
            )
        })?;
    let resting_order_refresh_margin_ns = economics
        .resting_order_refresh_margin_ms
        .checked_mul(NANOS_PER_MILLI_U64)
        .ok_or_else(|| {
            binding_message(
                context,
                "execution economics resting refresh margin overflows nanoseconds".to_string(),
            )
        })?;
    let carry_horizon_ns = economics
        .carry
        .as_ref()
        .map(|carry| {
            carry
                .holding_horizon_secs
                .checked_mul(MILLIS_PER_SECOND_U64)
                .and_then(|value| value.checked_mul(NANOS_PER_MILLI_U64))
                .ok_or_else(|| {
                    binding_message(
                        context,
                        "carry holding horizon overflows nanoseconds".to_string(),
                    )
                })
        })
        .transpose()?;
    let product_surface_routes = economics
        .product_surface_policies
        .iter()
        .map(|(product_surface_id, edge_basis_policy_id)| {
            let carry_plan = if economics.carry_surfaces.contains(product_surface_id) {
                BoltV3CarryPlan::Required {
                    holding_horizon_ns: carry_horizon_ns.ok_or_else(|| {
                        binding_message(
                            context,
                            "carry-bearing product surface has no carry policy".to_string(),
                        )
                    })?,
                }
            } else {
                BoltV3CarryPlan::NoCarry
            };
            Ok(crate::bolt_v3_order_execution::BoltV3ProductSurfaceRoute {
                product_surface_id,
                edge_basis_policy_id,
                carry_plan,
            })
        })
        .collect::<Result<Vec<_>, BoltV3StrategyRegistrationError>>()?;
    let source = ConfiguredEconomicsAdmissionSource::new(
        client.venue.as_str(),
        context.economics_inputs.clone(),
        ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns,
            quote_validity_ns,
            resting_order_refresh_margin_ns,
        },
    )
    .map_err(|error| binding_message(context, format!("economics source: {error}")))?;
    let account_id =
        execution_account_id(&context.loaded.root, execution_client_id).ok_or_else(|| {
            binding_message(
                context,
                "execution economics requires a configured account_id".to_string(),
            )
        })?;
    BoltV3OrderRoutingHandle::new_with_product_surfaces(
        Arc::new(source),
        crate::bolt_v3_order_execution::BoltV3MultiSurfaceOrderRoutingConfig {
            execution_client_id,
            account_id,
            product_surface_routes,
            reporting_policy_id: economics.reporting_policy.as_str(),
            reporting_unit: context
                .loaded
                .root
                .economics
                .reporting
                .pnl_currency
                .as_str(),
        },
    )
    .map_err(|error| binding_message(context, format!("economics routing: {error:#}")))
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
        .map(|pool| settlement_currency_from_config_code(pool.collateral_currency.as_str()))
}

pub(crate) fn settlement_currency_from_config_code(configured: &str) -> Currency {
    let pusd = Currency::pUSD();
    if configured.eq_ignore_ascii_case(pusd.code.as_str()) {
        pusd
    } else {
        Currency::from(configured)
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
                capabilities: binding.capabilities,
                resolved,
                decision_evidence: decision_evidence.clone(),
                submit_admission: execution_controls.submit_admission.clone(),
                iv_query_handles: iv_query_handles.clone(),
                order_execution_policy: execution_controls.order_execution_policy,
                realized_volatility_runtime: realized_volatility_runtime.clone(),
                settlement_runtime_sink: execution_controls.settlement_runtime_sink.clone(),
                settlement_recovery: execution_controls.settlement_recovery.clone(),
                settlement_health_transition_emitter: execution_controls
                    .settlement_health_transition_emitter
                    .clone(),
                economics_inputs: execution_controls.economics_inputs.clone(),
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
