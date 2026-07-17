//! Generic strategy registration boundary for bolt-v3.
//!
//! This module iterates validated bolt-v3 strategy envelopes and delegates
//! concrete registration to an injected binding. Concrete strategy builders
//! stay outside this core boundary.

use crate::bolt_v3_config::{
    BoltV3RootConfig, ClientBlock, LoadedBoltV3Config, LoadedStrategy, StrategyArchetypeKey,
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
use crate::bolt_v3_providers::{FeeProvider, resolve_fee_provider};
use crate::bolt_v3_secrets::ResolvedBoltV3Secrets;
use crate::bolt_v3_settlement_runtime::{
    BoltV3SettlementRecoveryConfig, BoltV3SettlementRuntimeSinkHandle,
};
use crate::bolt_v3_strategy_context::StrategyBuildContext;
use crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState;
use anyhow::Context;
use nautilus_common::{actor::DataActorNative, component::Component};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    identifiers::{ClientId, StrategyId, Venue},
    types::Currency,
};
use nautilus_system::trader::Trader;
use nautilus_trading::{Strategy, StrategyNative};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime;

trait PreparedStrategyCommit {
    fn prepare_registration(&mut self, trader: &Trader) -> anyhow::Result<StrategyId>;
    fn commit(self: Box<Self>, trader: &Rc<RefCell<Trader>>) -> anyhow::Result<()>;
}

struct PreparedConcreteStrategy<T> {
    strategy: T,
}

impl<T> PreparedStrategyCommit for PreparedConcreteStrategy<T>
where
    T: Strategy + StrategyNative + DataActorNative + Component + std::fmt::Debug + 'static,
{
    fn prepare_registration(&mut self, trader: &Trader) -> anyhow::Result<StrategyId> {
        trader.prepare_strategy_for_registration(&mut self.strategy)
    }

    fn commit(self: Box<Self>, trader: &Rc<RefCell<Trader>>) -> anyhow::Result<()> {
        let Self { strategy } = *self;
        trader.borrow_mut().add_strategy(strategy)
    }
}

pub struct PreparedStrategyRegistration {
    strategy_id: Option<StrategyId>,
    strategy: Box<dyn PreparedStrategyCommit>,
}

impl PreparedStrategyRegistration {
    pub fn from_strategy<T>(strategy: T) -> Self
    where
        T: Strategy + StrategyNative + DataActorNative + Component + std::fmt::Debug + 'static,
    {
        Self {
            strategy_id: None,
            strategy: Box::new(PreparedConcreteStrategy { strategy }),
        }
    }

    pub fn prepare_registration(&mut self, trader: &Trader) -> anyhow::Result<StrategyId> {
        let strategy_id = self.strategy.prepare_registration(trader)?;
        self.strategy_id = Some(strategy_id);
        Ok(strategy_id)
    }

    pub fn strategy_id(&self) -> Option<StrategyId> {
        self.strategy_id
    }

    pub fn commit(self, trader: &Rc<RefCell<Trader>>) -> anyhow::Result<StrategyId> {
        let strategy_id = self.strategy_id.context(stringify!(
            prepared_strategy_registration_has_no_preflighted_nt_strategy_id
        ))?;
        self.strategy.commit(trader)?;
        Ok(strategy_id)
    }
}

#[derive(Clone, Copy)]
pub struct StrategyRuntimeBinding {
    pub key: &'static str,
    pub strategy_kind: &'static str,
    pub capabilities: StrategyRuntimeCapabilities,
    pub prepare:
        for<'a> fn(
            StrategyRegistrationContext<'a>,
        )
            -> Result<PreparedStrategyRegistration, BoltV3StrategyRegistrationError>,
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
pub struct StrategyRegistrationRuntimeResources {
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    iv_query_handles: Arc<BoltV3IvQueryHandleRegistry>,
    realized_volatility_runtime: Arc<Mutex<RealizedVolSurfaceRuntime>>,
    execution_controls: BoltV3StrategyExecutionControls,
}

impl StrategyRegistrationRuntimeResources {
    pub fn new(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        iv_query_handles: Arc<BoltV3IvQueryHandleRegistry>,
        realized_volatility_runtime: Arc<Mutex<RealizedVolSurfaceRuntime>>,
        execution_controls: BoltV3StrategyExecutionControls,
    ) -> Self {
        Self {
            decision_evidence,
            iv_query_handles,
            realized_volatility_runtime,
            execution_controls,
        }
    }
}

#[derive(Clone)]
pub struct StrategyRegistrationContext<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy: &'a LoadedStrategy,
    pub strategy_kind: &'static str,
    pub capabilities: StrategyRuntimeCapabilities,
    pub decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    pub submit_admission: Arc<BoltV3SubmitAdmissionState>,
    pub iv_query_handles: Arc<BoltV3IvQueryHandleRegistry>,
    pub order_execution_policy: BoltV3OrderExecutionPolicy,
    realized_volatility_runtime: Option<Arc<Mutex<RealizedVolSurfaceRuntime>>>,
    client_routes: StrategyRegistrationClientRoutes,
    execution_venue: Venue,
    fee_provider: Arc<dyn FeeProvider>,
    settlement: Option<StrategyRegistrationSettlementResources>,
}

#[derive(Clone)]
struct StrategyRegistrationClientRoutes {
    venues_by_client_id: BTreeMap<ClientId, Venue>,
}

impl StrategyRegistrationClientRoutes {
    fn venue(&self, client_id: &ClientId) -> Option<Venue> {
        self.venues_by_client_id.get(client_id).copied()
    }
}

#[derive(Clone)]
struct StrategyRegistrationSettlementResources {
    settlement_account_id: String,
    settlement_currency: Currency,
    runtime_sink: Option<BoltV3SettlementRuntimeSinkHandle>,
    recovery: Option<BoltV3SettlementRecoveryConfig>,
    health_transition_emitter: Option<BoltV3SettlementHealthTransitionEmitter>,
}

#[derive(Clone)]
enum StrategyRegistrationSettlementIdentityError {
    AccountId { execution_client_id: String },
    Currency { settlement_account_id: String },
}

impl StrategyRegistrationSettlementIdentityError {
    fn message(&self) -> String {
        match self {
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
    pub fn new(
        loaded: &'a LoadedBoltV3Config,
        strategy: &'a LoadedStrategy,
        strategy_kind: &'static str,
        capabilities: StrategyRuntimeCapabilities,
        resolved: &'a ResolvedBoltV3Secrets,
        runtime_resources: StrategyRegistrationRuntimeResources,
    ) -> Result<Self, BoltV3StrategyRegistrationError> {
        let StrategyRegistrationRuntimeResources {
            decision_evidence,
            iv_query_handles,
            realized_volatility_runtime,
            execution_controls,
        } = runtime_resources;
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
        let execution_client_id = strategy.config.execution_client_id.as_str();
        let (client_routes, execution_client) = resolve_strategy_client_routes(loaded, strategy)?;
        let execution_venue = client_routes
            .venue(&strategy.config.execution_client_id)
            .ok_or_else(|| {
                binding_error(
                    strategy,
                    "prepared client routes did not retain the configured execution client",
                )
            })?;
        let settlement = capabilities
            .settlement
            .then(|| {
                resolve_settlement_capability(
                    loaded,
                    strategy,
                    execution_client,
                    execution_venue,
                    settlement_runtime_sink,
                    settlement_recovery,
                    settlement_health_transition_emitter,
                )
            })
            .transpose()
            .map_err(|error| binding_error(strategy, error.message()))?;
        let fee_provider = resolve_fee_provider(
            execution_client_id,
            execution_client,
            execution_venue,
            resolved,
        )
        .map_err(|error| binding_error(strategy, error.to_string()))?;

        Ok(Self {
            loaded,
            strategy,
            strategy_kind,
            capabilities,
            decision_evidence,
            submit_admission,
            iv_query_handles,
            order_execution_policy,
            realized_volatility_runtime,
            client_routes,
            execution_venue,
            fee_provider,
            settlement,
        })
    }

    pub(crate) fn prepared_client_venue(&self, client_id: &ClientId) -> Option<Venue> {
        self.client_routes.venue(client_id)
    }
}

fn resolve_strategy_client_routes<'a>(
    loaded: &'a LoadedBoltV3Config,
    strategy: &LoadedStrategy,
) -> Result<(StrategyRegistrationClientRoutes, &'a ClientBlock), BoltV3StrategyRegistrationError> {
    let mut roles_by_client_id = BTreeMap::<ClientId, BTreeSet<String>>::new();
    roles_by_client_id
        .entry(strategy.config.execution_client_id)
        .or_default()
        .insert(stringify!(execution_client_id).to_string());
    for (role, signal_data) in &strategy.config.signal_data {
        roles_by_client_id
            .entry(signal_data.data_client_id)
            .or_default()
            .insert(format!("signal_data.{role}.data_client_id"));
    }
    if let Some(resolution_data) = strategy.config.resolution_data.as_ref() {
        let mut resolution_role = stringify!(resolution_data).to_string();
        resolution_role.push('.');
        resolution_role.push_str(stringify!(data_client_id));
        roles_by_client_id
            .entry(resolution_data.data_client_id)
            .or_default()
            .insert(resolution_role);
    }

    let mut clients_by_id = BTreeMap::<ClientId, &'a ClientBlock>::new();
    let mut venues_by_client_id = BTreeMap::new();
    for (client_id, roles) in roles_by_client_id {
        let client = loaded.root.clients.get(client_id.as_str()).ok_or_else(|| {
            binding_error(
                strategy,
                format!(
                    "configured client `{client_id}` for {roles:?} is not present in loaded clients"
                ),
            )
        })?;
        clients_by_id.insert(client_id, client);
        venues_by_client_id.insert(client_id, client.venue);
    }

    let execution_client = clients_by_id
        .get(&strategy.config.execution_client_id)
        .copied()
        .ok_or_else(|| {
            binding_error(
                strategy,
                "prepared client routes did not retain the configured execution client",
            )
        })?;
    Ok((
        StrategyRegistrationClientRoutes {
            venues_by_client_id,
        },
        execution_client,
    ))
}

fn resolve_settlement_capability(
    loaded: &LoadedBoltV3Config,
    strategy: &LoadedStrategy,
    execution_client: &ClientBlock,
    execution_venue: Venue,
    runtime_sink: Option<BoltV3SettlementRuntimeSinkHandle>,
    recovery: Option<BoltV3SettlementRecoveryConfig>,
    health_transition_emitter: Option<BoltV3SettlementHealthTransitionEmitter>,
) -> Result<StrategyRegistrationSettlementResources, StrategyRegistrationSettlementIdentityError> {
    let execution_client_id = strategy.config.execution_client_id.as_str();
    let settlement_account_id =
        execution_account_id_from_client(execution_client).ok_or_else(|| {
            StrategyRegistrationSettlementIdentityError::AccountId {
                execution_client_id: execution_client_id.to_string(),
            }
        })?;
    let settlement_currency = settlement_currency_for_execution_account(
        &loaded.root,
        execution_venue,
        settlement_account_id,
    )
    .ok_or_else(|| StrategyRegistrationSettlementIdentityError::Currency {
        settlement_account_id: settlement_account_id.to_string(),
    })?;
    Ok(StrategyRegistrationSettlementResources {
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
    let execution_venue = context.execution_venue;
    let settlement = settlement_resources_for_context(context);
    let fee_provider = context.fee_provider.clone();
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

fn settlement_resources_for_context<'a>(
    context: &'a StrategyRegistrationContext<'_>,
) -> Option<&'a StrategyRegistrationSettlementResources> {
    context.settlement.as_ref()
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
    execution_account_id_from_client(root.clients.get(execution_client_id)?)
}

fn execution_account_id_from_client(client: &ClientBlock) -> Option<&str> {
    client
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

fn binding_error(strategy: &LoadedStrategy, message: String) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::Binding {
        strategy_instance_id: strategy.config.strategy_instance_id.clone(),
        strategy_archetype: strategy.config.strategy_archetype.as_str().to_string(),
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
    let runtime_resources = StrategyRegistrationRuntimeResources::new(
        decision_evidence,
        iv_query_handles,
        realized_volatility_runtime,
        execution_controls,
    );

    let contexts = loaded
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
                runtime_resources.clone(),
            )?;
            Ok((binding, context))
        })
        .collect::<Result<Vec<_>, BoltV3StrategyRegistrationError>>()?;

    let mut prepared = contexts
        .into_iter()
        .map(|(binding, context)| {
            let strategy = context.strategy;
            let prepared = (binding.prepare)(context)?;
            Ok((strategy, prepared))
        })
        .collect::<Result<Vec<_>, BoltV3StrategyRegistrationError>>()?;

    let mut prepared_strategy_ids = BTreeSet::new();
    let mut prepared_order_id_tags = BTreeSet::new();
    {
        let trader = node.kernel().trader().borrow();
        for (strategy, prepared_registration) in &mut prepared {
            let strategy_id = prepared_registration
                .prepare_registration(&trader)
                .map_err(|error| binding_error(strategy, error.to_string()))?;
            if !prepared_strategy_ids.insert(strategy_id) {
                return Err(binding_error(
                    strategy,
                    format!("prepared NT strategy ID `{strategy_id}` is duplicated in the batch"),
                ));
            }
            let order_id_tag = strategy_id.get_tag().to_string();
            if !prepared_order_id_tags.insert(order_id_tag.clone()) {
                return Err(binding_error(
                    strategy,
                    format!("prepared NT order ID tag `{order_id_tag}` is duplicated in the batch"),
                ));
            }
        }
    }

    for (strategy, prepared_registration) in prepared {
        // All repository-owned parsing, client resolution, registry selection,
        // construction, and identity checks have completed. NT add_strategy is
        // the external commit boundary and is intentionally not wrapped in a
        // repository-level rollback path.
        let registered_strategy_id = prepared_registration
            .commit(node.kernel().trader())
            .map_err(|error| binding_error(strategy, error.to_string()))?;
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
