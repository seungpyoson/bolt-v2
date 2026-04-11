use std::{
    any::Any,
    cell::RefCell,
    fmt,
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use nautilus_common::{
    actor::{DataActor, DataActorConfig, DataActorCore},
    component::Component,
    msgbus::{self, Endpoint, MStr, ShareableMessageHandler, TypedHandler, get_message_bus},
    nautilus_actor,
};
use nautilus_core::UUID4;
use nautilus_live::node::{LiveNode, LiveNodeHandle, NodeState};
use nautilus_model::identifiers::{ActorId, ClientId, InstrumentId, StrategyId};
use nautilus_system::trader::Trader;
use tokio::{
    task::JoinHandle,
    time::{MissedTickBehavior, interval, sleep},
};
use tokio_util::sync::CancellationToken;
use toml::Value;

use crate::{
    clients::{self, ReferenceDataClientParts},
    config::{Config, ReferenceVenueEntry, ReferenceVenueKind, RulesetConfig},
    platform::{
        audit::{
            AuditReceiver, AuditRecord, AuditSender, AuditSpoolConfig, AwsCliUploader,
            ReferenceVenueSnapshot, SelectorState, VenueHealthState, VenueKindState,
            spawn_audit_worker,
        },
        polymarket_catalog::load_candidate_markets_for_ruleset,
        reference::{ReferenceSnapshot, VenueHealth, VenueKind},
        reference_actor::{ReferenceActor, ReferenceActorConfig, ReferenceSubscription},
        ruleset::{
            CandidateMarket, SelectionDecision, SelectionEvaluation, SelectionState,
            evaluate_market_selection,
        },
    },
    strategies::exec_tester::build_exec_tester,
};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

pub type CandidateMarketLoadFuture = BoxFuture<Result<Vec<CandidateMarket>>>;

pub trait CandidateMarketLoader: Send + Sync + 'static {
    fn load(&self, ruleset: RulesetConfig) -> CandidateMarketLoadFuture;
}

impl<F, Fut> CandidateMarketLoader for F
where
    F: Fn(RulesetConfig) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<CandidateMarket>>> + Send + 'static,
{
    fn load(&self, ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
        Box::pin(self(ruleset))
    }
}

pub trait PlatformAuditTaskFactory: Send + Sync + 'static {
    fn spawn(
        &self,
        audit_rx: AuditReceiver,
        audit_config: AuditSpoolConfig,
        cancellation: CancellationToken,
    ) -> JoinHandle<Result<()>>;
}

#[derive(Clone)]
pub struct PlatformRuntimeServices {
    pub candidate_loader: Arc<dyn CandidateMarketLoader>,
    pub audit_task_factory: Arc<dyn PlatformAuditTaskFactory>,
    pub now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl PlatformRuntimeServices {
    pub fn production() -> Self {
        Self {
            candidate_loader: Arc::new(ProductionCandidateMarketLoader),
            audit_task_factory: Arc::new(ProductionAuditTaskFactory),
            now_ms: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock should be after UNIX_EPOCH")
                    .as_millis() as u64
            }),
        }
    }
}

pub struct PlatformRuntimeGuards {
    pub cancellation: CancellationToken,
    reference_snapshot_audit: Option<ReferenceSnapshotAuditSubscription>,
    runtime_strategy_applier_failure: Option<Arc<Mutex<Option<String>>>>,
    pub join_handles: Vec<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

#[derive(Clone)]
struct RuntimeStrategyTemplate {
    strategy_id: StrategyId,
    raw_config: Value,
}

#[derive(Clone, Debug)]
struct RuntimeManagedStrategy {
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
}

#[derive(Clone, Debug)]
enum RuntimeStrategyCommand {
    Activate { instrument_id: String },
    Clear,
}

#[derive(Clone, Debug)]
struct RuntimeStrategyApplierConfig {
    base: DataActorConfig,
}

struct RuntimeStrategyApplier {
    core: DataActorCore,
    config: RuntimeStrategyApplierConfig,
    state: Rc<RefCell<RuntimeStrategyApplierState>>,
}

struct RuntimeStrategyApplierState {
    template: RuntimeStrategyTemplate,
    trader: Rc<RefCell<Trader>>,
    active_runtime_strategy: Option<RuntimeManagedStrategy>,
    failure: Arc<Mutex<Option<String>>>,
    cancellation: CancellationToken,
    node_handle: LiveNodeHandle,
}

impl fmt::Debug for RuntimeStrategyApplier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let active_runtime_strategy = self
            .state
            .try_borrow()
            .ok()
            .and_then(|state| state.active_runtime_strategy.clone());
        f.debug_struct("RuntimeStrategyApplier")
            .field("config", &self.config)
            .field("active_runtime_strategy", &active_runtime_strategy)
            .finish()
    }
}

impl RuntimeStrategyApplier {
    fn new(
        config: RuntimeStrategyApplierConfig,
        template: RuntimeStrategyTemplate,
        trader: Rc<RefCell<Trader>>,
        failure: Arc<Mutex<Option<String>>>,
        cancellation: CancellationToken,
        node_handle: LiveNodeHandle,
    ) -> Self {
        Self {
            core: DataActorCore::new(config.base.clone()),
            config,
            state: Rc::new(RefCell::new(RuntimeStrategyApplierState {
                template,
                trader,
                active_runtime_strategy: None,
                failure,
                cancellation,
                node_handle,
            })),
        }
    }

    fn execute_endpoint(actor_id: &ActorId) -> MStr<Endpoint> {
        format!("{actor_id}.runtime-strategy-apply").into()
    }

    fn send(actor_id: &ActorId, command: RuntimeStrategyCommand) -> Result<()> {
        let endpoint = Self::execute_endpoint(actor_id);
        let handler = {
            let msgbus = get_message_bus();
            msgbus
                .borrow_mut()
                .endpoint_map::<RuntimeStrategyCommand>()
                .get(endpoint)
                .cloned()
        };

        let Some(handler) = handler else {
            bail!(
                "runtime strategy apply endpoint '{}' not registered",
                endpoint.as_str()
            );
        };

        handler.handle(&command);
        Ok(())
    }

    fn register_execute_endpoint(&self) {
        let actor_id = self.actor_id();
        let state = Rc::clone(&self.state);
        let handler = TypedHandler::from(move |command: &RuntimeStrategyCommand| {
            let (result, node_handle, cancellation) = {
                let mut state = state.borrow_mut();
                let node_handle = state.node_handle.clone();
                let cancellation = state.cancellation.clone();
                let result = state.execute(command.clone()).map_err(|error| {
                    let error_message = error
                        .context("runtime-managed strategy apply failed")
                        .to_string();
                    state.record_failure(error_message.clone());
                    error_message
                });
                (result, node_handle, cancellation)
            };

            if let Err(error_message) = result {
                let _ = fail_closed(&node_handle, &cancellation, anyhow!(error_message));
            }
        });

        get_message_bus()
            .borrow_mut()
            .endpoint_map::<RuntimeStrategyCommand>()
            .register(Self::execute_endpoint(&actor_id), handler);
    }

    fn deregister_execute_endpoint(&self) {
        let actor_id = self.actor_id();
        get_message_bus()
            .borrow_mut()
            .endpoint_map::<RuntimeStrategyCommand>()
            .deregister(Self::execute_endpoint(&actor_id));
    }
}

impl RuntimeStrategyApplierState {
    fn execute(&mut self, command: RuntimeStrategyCommand) -> Result<()> {
        let desired_instrument_id = match command {
            RuntimeStrategyCommand::Activate { instrument_id } => {
                Some(InstrumentId::from(instrument_id.as_str()))
            }
            RuntimeStrategyCommand::Clear => None,
        };

        reconcile_runtime_strategy(
            &self.trader,
            &self.template,
            desired_instrument_id,
            &mut self.active_runtime_strategy,
        )
    }

    fn record_failure(&self, error_message: String) {
        if let Ok(mut failure) = self.failure.lock()
            && failure.is_none()
        {
            *failure = Some(error_message);
        }
    }
}

nautilus_actor!(RuntimeStrategyApplier);

impl DataActor for RuntimeStrategyApplier {
    fn on_start(&mut self) -> Result<()> {
        self.register_execute_endpoint();
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        self.deregister_execute_endpoint();
        Ok(())
    }
}

impl PlatformRuntimeGuards {
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.cancellation.cancel();

        let mut first_error = None;
        if let Some(reference_snapshot_audit) = self.reference_snapshot_audit.take()
            && let Err(error) = reference_snapshot_audit.unsubscribe()
        {
            first_error = Some(error);
        }
        if let Some(runtime_strategy_applier_failure) = self.runtime_strategy_applier_failure.take()
        {
            let error_message = runtime_strategy_applier_failure
                .lock()
                .expect("runtime strategy applier failure mutex poisoned")
                .take();
            if let Some(error_message) = error_message
                && first_error.is_none()
            {
                first_error = Some(anyhow!(error_message));
            }
        }
        for handle in self.join_handles {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) if first_error.is_none() => first_error = Some(error),
                Ok(Err(_)) => {}
                Err(error) if first_error.is_none() => {
                    first_error = Some(anyhow!("platform runtime task join failed: {error}"));
                }
                Err(_) => {}
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

pub fn build_reference_data_client(
    venue: &ReferenceVenueEntry,
) -> Result<ReferenceDataClientParts, Box<dyn std::error::Error>> {
    match &venue.kind {
        ReferenceVenueKind::Binance => Ok(clients::binance::build_reference_data_client()),
        ReferenceVenueKind::Bybit => Ok(clients::bybit::build_reference_data_client()),
        ReferenceVenueKind::Deribit => Ok(clients::deribit::build_reference_data_client()),
        ReferenceVenueKind::Hyperliquid => Ok(clients::hyperliquid::build_reference_data_client()),
        ReferenceVenueKind::Kraken => Ok(clients::kraken::build_reference_data_client()),
        ReferenceVenueKind::Okx => Ok(clients::okx::build_reference_data_client()),
        other => Err(format!("unsupported reference venue kind: {other:?}").into()),
    }
}

pub fn wire_platform_runtime(
    node: &mut LiveNode,
    cfg: &Config,
) -> anyhow::Result<PlatformRuntimeGuards> {
    wire_platform_runtime_with_services(node, cfg, PlatformRuntimeServices::production())
}

pub fn wire_platform_runtime_with_services(
    node: &mut LiveNode,
    cfg: &Config,
    services: PlatformRuntimeServices,
) -> anyhow::Result<PlatformRuntimeGuards> {
    if cfg.rulesets.is_empty() {
        bail!("platform runtime requires at least one active ruleset");
    }

    let ruleset = cfg
        .rulesets
        .first()
        .cloned()
        .context("platform runtime requires one active ruleset")?;
    let runtime_strategy_template = runtime_strategy_template(cfg)?;
    let runtime_strategy_applier_failure = Arc::new(Mutex::new(None::<String>));
    let selector_poll_interval = Duration::from_millis(ruleset.selector_poll_interval_ms);
    let cancellation = CancellationToken::new();
    let audit_cfg = cfg
        .audit
        .as_ref()
        .context("platform runtime requires audit configuration")?;

    add_reference_actor(node, cfg)?;
    let runtime_strategy_actor_id =
        if let Some(runtime_strategy_template) = runtime_strategy_template {
            Some(add_runtime_strategy_applier_actor(
                node,
                runtime_strategy_template,
                Arc::clone(&runtime_strategy_applier_failure),
                node.handle(),
                cancellation.clone(),
            )?)
        } else {
            None
        };
    let (audit_tx, audit_rx) = crate::platform::audit::audit_channel();
    let audit_config = AuditSpoolConfig {
        spool_dir: audit_cfg.local_dir.clone().into(),
        s3_prefix: audit_cfg.s3_uri.clone(),
        node_name: cfg.node.name.clone(),
        run_id: node.instance_id().to_string(),
        ship_interval: Duration::from_secs(audit_cfg.ship_interval_secs),
        upload_attempt_timeout: Duration::from_secs(audit_cfg.upload_attempt_timeout_secs),
        roll_max_bytes: audit_cfg.roll_max_bytes,
        roll_max_secs: audit_cfg.roll_max_secs,
        max_local_backlog_bytes: audit_cfg.max_local_backlog_bytes,
    };
    let audit_task = tokio::spawn(run_audit_task(
        services.audit_task_factory,
        audit_rx,
        audit_config,
        cancellation.clone(),
        node.handle(),
    ));
    let reference_snapshot_audit = subscribe_reference_snapshot_audit(
        cfg.reference.publish_topic.clone(),
        audit_tx.clone(),
        cancellation.clone(),
        node.handle(),
    );
    let selector_task = spawn_selector_task(
        runtime_strategy_actor_id.is_some(),
        run_selector_task(
            ruleset,
            selector_poll_interval,
            services.candidate_loader,
            services.now_ms,
            audit_tx,
            runtime_strategy_actor_id,
            cancellation.clone(),
            node.handle(),
        ),
    );

    Ok(PlatformRuntimeGuards {
        cancellation,
        reference_snapshot_audit: Some(reference_snapshot_audit),
        runtime_strategy_applier_failure: Some(runtime_strategy_applier_failure),
        join_handles: vec![audit_task, selector_task],
    })
}

fn add_reference_actor(node: &mut LiveNode, cfg: &Config) -> Result<()> {
    let venue_subscriptions = cfg
        .reference
        .venues
        .iter()
        .map(|venue| {
            Ok(ReferenceSubscription {
                venue_name: venue.name.clone(),
                instrument_id: InstrumentId::from(venue.instrument_id.as_str()),
                client_id: client_id_for_reference_venue(cfg, venue)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let venue_cfgs = cfg.reference.venues.clone();

    let actor = ReferenceActor::new(
        ReferenceActorConfig {
            base: DataActorConfig {
                actor_id: Some(ActorId::from(
                    format!("REFERENCE-ACTOR-{}", UUID4::new()).as_str(),
                )),
                ..Default::default()
            },
            publish_topic: cfg.reference.publish_topic.clone(),
            min_publish_interval_ms: cfg.reference.min_publish_interval_ms,
            venue_subscriptions,
        },
        venue_cfgs,
    );

    node.add_actor(actor)
        .context("failed to register reference actor")?;
    Ok(())
}

fn add_runtime_strategy_applier_actor(
    node: &mut LiveNode,
    template: RuntimeStrategyTemplate,
    failure: Arc<Mutex<Option<String>>>,
    node_handle: LiveNodeHandle,
    cancellation: CancellationToken,
) -> Result<ActorId> {
    let actor = RuntimeStrategyApplier::new(
        RuntimeStrategyApplierConfig {
            base: DataActorConfig {
                actor_id: Some(ActorId::from(
                    format!("RUNTIME-STRATEGY-APPLIER-{}", UUID4::new()).as_str(),
                )),
                ..Default::default()
            },
        },
        template,
        Rc::clone(node.kernel().trader()),
        failure,
        cancellation,
        node_handle,
    );
    let actor_id = actor.actor_id();

    node.add_actor(actor)
        .context("failed to register runtime strategy applier actor")?;
    Ok(actor_id)
}

fn client_id_for_reference_venue(cfg: &Config, venue: &ReferenceVenueEntry) -> Result<ClientId> {
    Ok(ClientId::from(
        reference_client_name_for_kind(cfg, &venue.kind)?.as_str(),
    ))
}

pub fn reference_client_name_for_kind(cfg: &Config, kind: &ReferenceVenueKind) -> Result<String> {
    match kind {
        ReferenceVenueKind::Binance => Ok("BINANCE".to_string()),
        ReferenceVenueKind::Bybit => Ok("BYBIT".to_string()),
        ReferenceVenueKind::Deribit => Ok("DERIBIT".to_string()),
        ReferenceVenueKind::Hyperliquid => Ok("HYPERLIQUID".to_string()),
        ReferenceVenueKind::Kraken => Ok("KRAKEN".to_string()),
        ReferenceVenueKind::Okx => Ok("OKX".to_string()),
        ReferenceVenueKind::Chainlink => Ok("CHAINLINK".to_string()),
        ReferenceVenueKind::Polymarket => cfg
            .data_clients
            .iter()
            .find(|client| client.kind == "polymarket")
            .map(|client| client.name.clone())
            .context("reference polymarket venue requires the primary polymarket data client"),
    }
}

async fn run_audit_task(
    audit_task_factory: Arc<dyn PlatformAuditTaskFactory>,
    audit_rx: AuditReceiver,
    audit_config: AuditSpoolConfig,
    cancellation: CancellationToken,
    node_handle: LiveNodeHandle,
) -> Result<()> {
    if !wait_for_node_running(&node_handle, &cancellation).await {
        return Ok(());
    }

    let audit_task = audit_task_factory.spawn(audit_rx, audit_config, cancellation.clone());
    supervise_audit_task(audit_task, cancellation, node_handle).await
}

fn spawn_selector_task<F>(uses_local_task: bool, task: F) -> JoinHandle<Result<()>>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    if uses_local_task {
        tokio::task::spawn_local(task)
    } else {
        tokio::spawn(task)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_selector_task(
    ruleset: RulesetConfig,
    poll_interval: Duration,
    selector_loader: Arc<dyn CandidateMarketLoader>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    audit_tx: AuditSender,
    runtime_strategy_actor_id: Option<ActorId>,
    cancellation: CancellationToken,
    node_handle: LiveNodeHandle,
) -> Result<()> {
    if !wait_for_node_running(&node_handle, &cancellation).await {
        return Ok(());
    }

    let mut ticker = interval(poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        if !node_is_running(&node_handle) {
            return Ok(());
        }

        tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            _ = ticker.tick() => {}
        }

        if !node_is_running(&node_handle) {
            return Ok(());
        }

        let load_future = selector_loader.load(ruleset.clone());
        tokio::pin!(load_future);
        let candidates = match tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            result = &mut load_future => result,
        } {
            Ok(candidates) => candidates,
            Err(error) => {
                return Err(fail_closed(
                    &node_handle,
                    &cancellation,
                    error.context("selector polling failed"),
                ));
            }
        };

        if !node_is_running(&node_handle) {
            return Ok(());
        }

        let evaluation = evaluate_market_selection(&ruleset, &candidates);
        if let Err(error) = send_selector_evaluation(&audit_tx, &evaluation, &now_ms) {
            if cancellation.is_cancelled() {
                return Ok(());
            }

            return Err(fail_closed(
                &node_handle,
                &cancellation,
                error.context("selector audit send failed"),
            ));
        }

        if let Some(runtime_strategy_actor_id) = runtime_strategy_actor_id.as_ref()
            && let Err(error) =
                apply_runtime_strategy_command(runtime_strategy_actor_id, &evaluation.decision)
        {
            if cancellation.is_cancelled() {
                return Ok(());
            }

            return Err(fail_closed(
                &node_handle,
                &cancellation,
                error.context("runtime-managed strategy command send failed"),
            ));
        }
    }
}

fn runtime_strategy_template(cfg: &Config) -> Result<Option<RuntimeStrategyTemplate>> {
    let matching_templates: Vec<_> = cfg
        .strategies
        .iter()
        .filter(|strategy| strategy.kind == "exec_tester")
        .collect();

    let strategy = match matching_templates.as_slice() {
        [] => return Ok(None),
        [strategy] => *strategy,
        _ => {
            bail!(
                "platform runtime supports at most one exec_tester strategy template, got {}",
                matching_templates.len()
            );
        }
    };

    let strategy_id = strategy
        .config
        .as_table()
        .and_then(|table| table.get("strategy_id"))
        .and_then(Value::as_str)
        .context("exec_tester strategy template must include strategy_id")?;

    Ok(Some(RuntimeStrategyTemplate {
        strategy_id: StrategyId::from(strategy_id),
        raw_config: strategy.config.clone(),
    }))
}

fn reconcile_runtime_strategy(
    trader: &Rc<RefCell<Trader>>,
    template: &RuntimeStrategyTemplate,
    desired_instrument_id: Option<InstrumentId>,
    active_runtime_strategy: &mut Option<RuntimeManagedStrategy>,
) -> Result<()> {
    let template_strategy_registered = trader
        .borrow()
        .strategy_ids()
        .contains(&template.strategy_id);
    let registered_strategy_id = active_runtime_strategy
        .as_ref()
        .filter(|strategy| {
            trader
                .borrow()
                .strategy_ids()
                .contains(&strategy.strategy_id)
        })
        .map(|strategy| strategy.strategy_id);

    match desired_instrument_id {
        Some(desired_instrument_id) => {
            if active_runtime_strategy.as_ref().is_some_and(|strategy| {
                strategy.instrument_id == desired_instrument_id
                    && Some(strategy.strategy_id) == registered_strategy_id
            }) {
                return Ok(());
            }

            if let Some(strategy_id) = registered_strategy_id {
                trader
                    .borrow_mut()
                    .remove_strategy(&strategy_id)
                    .with_context(|| {
                        format!(
                            "failed removing runtime-managed strategy {strategy_id} before switch"
                        )
                    })?;
                *active_runtime_strategy = None;
            }

            if template_strategy_registered && registered_strategy_id.is_none() {
                trader
                    .borrow_mut()
                    .remove_strategy(&template.strategy_id)
                    .with_context(|| {
                        format!(
                            "failed removing pre-registered template strategy {} before activation",
                            template.strategy_id
                        )
                    })?;
            }

            let strategy = build_runtime_exec_tester(template, desired_instrument_id)?;
            let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());

            trader
                .borrow_mut()
                .add_strategy(strategy)
                .with_context(|| {
                    format!("failed registering runtime-managed strategy {strategy_id}")
                })?;
            trader
                .borrow()
                .start_strategy(&strategy_id)
                .with_context(|| {
                    format!("failed starting runtime-managed strategy {strategy_id}")
                })?;

            *active_runtime_strategy = Some(RuntimeManagedStrategy {
                strategy_id,
                instrument_id: desired_instrument_id,
            });
        }
        None => {
            if let Some(strategy_id) = registered_strategy_id {
                trader
                    .borrow_mut()
                    .remove_strategy(&strategy_id)
                    .with_context(|| {
                        format!("failed removing runtime-managed strategy {strategy_id}")
                    })?;
            }
            if template_strategy_registered && registered_strategy_id.is_none() {
                trader
                    .borrow_mut()
                    .remove_strategy(&template.strategy_id)
                    .with_context(|| {
                        format!(
                            "failed removing pre-registered template strategy {}",
                            template.strategy_id
                        )
                    })?;
            }
            *active_runtime_strategy = None;
        }
    }

    Ok(())
}

fn apply_runtime_strategy_command(
    runtime_strategy_actor_id: &ActorId,
    decision: &SelectionDecision,
) -> Result<()> {
    let command = match &decision.state {
        SelectionState::Active { market } => RuntimeStrategyCommand::Activate {
            instrument_id: market.instrument_id.clone(),
        },
        SelectionState::Idle { .. } | SelectionState::Freeze { .. } => {
            RuntimeStrategyCommand::Clear
        }
    };

    RuntimeStrategyApplier::send(runtime_strategy_actor_id, command)
}

fn build_runtime_exec_tester(
    template: &RuntimeStrategyTemplate,
    instrument_id: InstrumentId,
) -> Result<nautilus_testkit::testers::ExecTester> {
    let mut raw_config = template.raw_config.clone();
    let table = raw_config
        .as_table_mut()
        .context("exec_tester strategy template config must be a TOML table")?;
    table.insert(
        "instrument_id".to_string(),
        Value::String(instrument_id.to_string()),
    );

    build_exec_tester(&raw_config).map_err(|error| anyhow!(error.to_string()))
}

struct ReferenceSnapshotAuditSubscription {
    publish_topic: String,
    handler: ShareableMessageHandler,
    send_failure: Arc<Mutex<Option<String>>>,
}

impl ReferenceSnapshotAuditSubscription {
    fn unsubscribe(self) -> Result<()> {
        msgbus::unsubscribe_any(self.publish_topic.into(), &self.handler);

        let error_message = self
            .send_failure
            .lock()
            .expect("snapshot send failure mutex poisoned")
            .take();
        match error_message {
            Some(error_message) => Err(anyhow!(error_message)),
            None => Ok(()),
        }
    }
}

fn subscribe_reference_snapshot_audit(
    publish_topic: String,
    audit_tx: AuditSender,
    cancellation: CancellationToken,
    node_handle: LiveNodeHandle,
) -> ReferenceSnapshotAuditSubscription {
    let send_failure = Arc::new(Mutex::new(None::<String>));
    let handler_cancellation = cancellation.clone();
    let handler_node_handle = node_handle.clone();
    let handler_audit_tx = audit_tx.clone();
    let handler_send_failure = Arc::clone(&send_failure);

    let handler = ShareableMessageHandler::from_any(move |message: &dyn Any| {
        if handler_cancellation.is_cancelled() || !node_is_running(&handler_node_handle) {
            return;
        }

        let Some(snapshot) = message.downcast_ref::<ReferenceSnapshot>() else {
            return;
        };

        if let Err(error) = send_reference_snapshot(&handler_audit_tx, snapshot) {
            let error = error.context("reference snapshot audit send failed");
            let error_message = error.to_string();
            if let Ok(mut send_failure) = handler_send_failure.lock()
                && send_failure.is_none()
            {
                *send_failure = Some(error_message.clone());
            }
            let _ = fail_closed(
                &handler_node_handle,
                &handler_cancellation,
                anyhow!(error_message),
            );
        }
    });

    msgbus::subscribe_any(publish_topic.clone().into(), handler.clone(), None);

    ReferenceSnapshotAuditSubscription {
        publish_topic,
        handler,
        send_failure,
    }
}

async fn supervise_audit_task(
    audit_task: JoinHandle<Result<()>>,
    cancellation: CancellationToken,
    node_handle: LiveNodeHandle,
) -> Result<()> {
    match audit_task.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(fail_closed(
            &node_handle,
            &cancellation,
            error.context("platform audit task failed"),
        )),
        Err(error) => Err(fail_closed(
            &node_handle,
            &cancellation,
            anyhow!("platform audit task join failed: {error}"),
        )),
    }
}

fn send_selector_evaluation(
    audit_tx: &AuditSender,
    evaluation: &SelectionEvaluation,
    now_ms: &Arc<dyn Fn() -> u64 + Send + Sync>,
) -> Result<()> {
    let ts_ms = now_ms();

    for rejected_candidate in &evaluation.rejected_candidates {
        audit_tx
            .send(AuditRecord::EligibilityReject {
                ts_ms,
                ruleset_id: evaluation.decision.ruleset_id.clone(),
                market_id: rejected_candidate.market.market_id.clone(),
                instrument_id: rejected_candidate.market.instrument_id.clone(),
                reason: rejected_candidate.reason.as_str().to_string(),
            })
            .map_err(|_| anyhow!("audit channel is closed"))?;
    }

    send_selector_decision(audit_tx, &evaluation.decision, ts_ms)
}

fn send_selector_decision(
    audit_tx: &AuditSender,
    decision: &SelectionDecision,
    ts_ms: u64,
) -> Result<()> {
    let record = match &decision.state {
        SelectionState::Active { market } => AuditRecord::SelectorDecision {
            ts_ms,
            ruleset_id: decision.ruleset_id.clone(),
            state: SelectorState::Active,
            market_id: Some(market.market_id.clone()),
            instrument_id: Some(market.instrument_id.clone()),
            reason: None,
        },
        SelectionState::Freeze { market, reason } => AuditRecord::SelectorDecision {
            ts_ms,
            ruleset_id: decision.ruleset_id.clone(),
            state: SelectorState::Freeze,
            market_id: Some(market.market_id.clone()),
            instrument_id: Some(market.instrument_id.clone()),
            reason: Some(reason.clone()),
        },
        SelectionState::Idle { reason } => AuditRecord::SelectorDecision {
            ts_ms,
            ruleset_id: decision.ruleset_id.clone(),
            state: SelectorState::Idle,
            market_id: None,
            instrument_id: None,
            reason: Some(reason.clone()),
        },
    };

    audit_tx
        .send(record)
        .map_err(|_| anyhow!("audit channel is closed"))?;
    Ok(())
}

fn send_reference_snapshot(audit_tx: &AuditSender, snapshot: &ReferenceSnapshot) -> Result<()> {
    audit_tx
        .send(AuditRecord::ReferenceSnapshot {
            ts_ms: snapshot.ts_ms,
            topic: snapshot.topic.clone(),
            fair_value: snapshot.fair_value,
            confidence: snapshot.confidence,
            venues: snapshot
                .venues
                .iter()
                .map(|venue| ReferenceVenueSnapshot {
                    venue_name: venue.venue_name.clone(),
                    base_weight: venue.base_weight,
                    effective_weight: venue.effective_weight,
                    stale: venue.stale,
                    health: match &venue.health {
                        VenueHealth::Healthy => VenueHealthState::Healthy,
                        VenueHealth::Disabled { .. } => VenueHealthState::Disabled,
                    },
                    reason: match &venue.health {
                        VenueHealth::Healthy => None,
                        VenueHealth::Disabled { reason } => Some(reason.clone()),
                    },
                    observed_ts_ms: venue.observed_ts_ms,
                    venue_kind: match venue.venue_kind {
                        VenueKind::Orderbook => VenueKindState::Orderbook,
                        VenueKind::Oracle => VenueKindState::Oracle,
                    },
                    observed_price: venue.observed_price,
                    observed_bid: venue.observed_bid,
                    observed_ask: venue.observed_ask,
                })
                .collect(),
        })
        .map_err(|_| anyhow!("audit channel is closed"))?;
    Ok(())
}

fn fail_closed(
    node_handle: &LiveNodeHandle,
    cancellation: &CancellationToken,
    error: anyhow::Error,
) -> anyhow::Error {
    cancellation.cancel();
    node_handle.stop();
    log::error!("{error}");
    error
}

fn node_is_running(node_handle: &LiveNodeHandle) -> bool {
    matches!(node_handle.state(), NodeState::Running)
}

async fn wait_for_node_running(
    node_handle: &LiveNodeHandle,
    cancellation: &CancellationToken,
) -> bool {
    loop {
        if node_is_running(node_handle) {
            return true;
        }

        match node_handle.state() {
            NodeState::ShuttingDown | NodeState::Stopped => return false,
            NodeState::Idle | NodeState::Starting | NodeState::Running => {}
        }

        tokio::select! {
            _ = cancellation.cancelled() => return false,
            _ = sleep(Duration::from_millis(10)) => {}
        }
    }
}

struct ProductionCandidateMarketLoader;

impl CandidateMarketLoader for ProductionCandidateMarketLoader {
    fn load(&self, ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
        Box::pin(async move {
            load_candidate_markets_for_ruleset(&ruleset, ruleset.candidate_load_timeout_secs)
                .await
                .context("failed to load candidate markets for ruleset")
        })
    }
}

struct ProductionAuditTaskFactory;

impl PlatformAuditTaskFactory for ProductionAuditTaskFactory {
    fn spawn(
        &self,
        audit_rx: AuditReceiver,
        audit_config: AuditSpoolConfig,
        cancellation: CancellationToken,
    ) -> JoinHandle<Result<()>> {
        let worker = spawn_audit_worker(audit_rx, AwsCliUploader, audit_config);
        tokio::spawn(async move { worker.run_until_cancelled(cancellation).await })
    }
}
