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
    msgbus::{self, ShareableMessageHandler},
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
    clients::{self, ReferenceDataClientParts, polymarket::PolymarketSelectorState},
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
            CandidateMarket, RuntimeSelectionSnapshot, SelectionDecision, SelectionEvaluation,
            SelectionState, evaluate_market_selection,
        },
    },
    strategies::registry::{StrategyBuildContext, StrategyRegistry},
};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

pub type CandidateMarketLoadFuture = BoxFuture<Result<Vec<CandidateMarket>>>;
pub type RuntimeStrategyFactory =
    Arc<dyn Fn(&Rc<RefCell<Trader>>, &str, &Value) -> Result<StrategyId> + Send + Sync>;

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
    pub runtime_strategy_factory: RuntimeStrategyFactory,
}

impl PlatformRuntimeServices {
    pub fn production(
        runtime_strategy_factory: RuntimeStrategyFactory,
        selector_state: Option<PolymarketSelectorState>,
        gamma_event_fetch_max_concurrent: usize,
    ) -> Self {
        Self {
            candidate_loader: Arc::new(ProductionCandidateMarketLoader {
                selector_state,
                gamma_event_fetch_max_concurrent,
            }),
            audit_task_factory: Arc::new(ProductionAuditTaskFactory),
            now_ms: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock should be after UNIX_EPOCH")
                    .as_millis() as u64
            }),
            runtime_strategy_factory,
        }
    }
}

pub struct PlatformRuntimeGuards {
    pub cancellation: CancellationToken,
    reference_snapshot_audit: Option<ReferenceSnapshotAuditSubscription>,
    runtime_strategy_shutdown: Option<RuntimeStrategyShutdown>,
    runtime_strategy_applier_failure: Option<Arc<Mutex<Option<String>>>>,
    pub join_handles: Vec<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

struct RuntimeStrategyShutdown {
    trader: Rc<RefCell<Trader>>,
    strategy_id: StrategyId,
    owned_by_runtime: bool,
}

impl RuntimeStrategyShutdown {
    fn remove(self) -> Result<()> {
        if !self.owned_by_runtime {
            return Ok(());
        }

        if self
            .trader
            .borrow()
            .strategy_ids()
            .contains(&self.strategy_id)
        {
            self.trader
                .borrow_mut()
                .remove_strategy(&self.strategy_id)
                .with_context(|| {
                    format!(
                        "failed removing runtime-managed strategy {} during shutdown",
                        self.strategy_id
                    )
                })?;
        }

        Ok(())
    }
}

#[derive(Clone)]
struct RuntimeStrategyTemplate {
    kind: String,
    strategy_id: StrategyId,
    raw_config: Value,
}

#[derive(Clone, Debug)]
struct RuntimeManagedStrategy {
    strategy_id: StrategyId,
    owned_by_runtime: bool,
}

#[derive(Clone, Debug)]
struct RuntimeStrategyApplierConfig {
    base: DataActorConfig,
    selection_topic: String,
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
    selection_handler: Option<ShareableMessageHandler>,
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
        active_runtime_strategy: RuntimeManagedStrategy,
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
                active_runtime_strategy: Some(active_runtime_strategy),
                failure,
                cancellation,
                node_handle,
                selection_handler: None,
            })),
        }
    }

    fn register_selection_subscription(&self) {
        let state = Rc::clone(&self.state);
        let topic = self.config.selection_topic.clone();
        let handler = ShareableMessageHandler::from_any(move |message: &dyn Any| {
            let Some(snapshot) = message.downcast_ref::<RuntimeSelectionSnapshot>() else {
                return;
            };
            let (result, node_handle, cancellation) = {
                let mut state = state.borrow_mut();
                let node_handle = state.node_handle.clone();
                let cancellation = state.cancellation.clone();
                let result = state.execute(snapshot.clone()).map_err(|error| {
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

        msgbus::subscribe_any(topic.into(), handler.clone(), None);
        self.state.borrow_mut().selection_handler = Some(handler);
    }

    fn deregister_selection_subscription(&self) {
        let handler = self.state.borrow_mut().selection_handler.take();
        if let Some(handler) = handler {
            msgbus::unsubscribe_any(self.config.selection_topic.clone().into(), &handler);
        }
    }
}

impl RuntimeStrategyApplierState {
    fn ensure_runtime_strategy_persisted(&mut self) -> Result<()> {
        let strategy_id = self
            .active_runtime_strategy
            .as_ref()
            .map(|strategy| strategy.strategy_id)
            .unwrap_or(self.template.strategy_id);

        if self.trader.borrow().strategy_ids().contains(&strategy_id) {
            if self.active_runtime_strategy.is_none() {
                self.active_runtime_strategy = Some(RuntimeManagedStrategy {
                    strategy_id,
                    owned_by_runtime: false,
                });
            }
            return Ok(());
        }

        if self.cancellation.is_cancelled() {
            return Ok(());
        }

        bail!("runtime-managed strategy {strategy_id} missing before shutdown");
    }

    fn execute(&mut self, _snapshot: RuntimeSelectionSnapshot) -> Result<()> {
        // Phase 1 keeps the runtime-owned strategy persistent for the full node run.
        // Selection snapshots are a liveness signal here; strategies that need the
        // payload must subscribe to the runtime selection topic directly.
        self.ensure_runtime_strategy_persisted()
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
        self.register_selection_subscription();
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        self.deregister_selection_subscription();
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
        if let Some(runtime_strategy_shutdown) = self.runtime_strategy_shutdown.take()
            && let Err(error) = runtime_strategy_shutdown.remove()
            && first_error.is_none()
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
    runtime_strategy_factory: RuntimeStrategyFactory,
    selector_state: Option<PolymarketSelectorState>,
) -> anyhow::Result<PlatformRuntimeGuards> {
    let gamma_event_fetch_max_concurrent = polymarket_gamma_event_fetch_max_concurrent(cfg)?;
    wire_platform_runtime_with_services(
        node,
        cfg,
        PlatformRuntimeServices::production(
            runtime_strategy_factory,
            selector_state,
            gamma_event_fetch_max_concurrent,
        ),
    )
}

fn polymarket_gamma_event_fetch_max_concurrent(cfg: &Config) -> anyhow::Result<usize> {
    let raw = cfg
        .data_clients
        .iter()
        .find(|client| client.kind == "polymarket")
        .ok_or_else(|| anyhow!("missing polymarket data client"))?
        .config
        .get("gamma_event_fetch_max_concurrent")
        .ok_or_else(|| {
            anyhow!(
                "polymarket data_client config is missing gamma_event_fetch_max_concurrent; render/validation must populate it"
            )
        })?;
    let value = raw
        .as_integer()
        .context("gamma_event_fetch_max_concurrent must be an integer")?;
    if value <= 0 {
        anyhow::bail!("gamma_event_fetch_max_concurrent must be > 0, got {value}");
    }
    usize::try_from(value).context("gamma_event_fetch_max_concurrent exceeds usize range")
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
    let runtime_selection_topic = runtime_strategy_template
        .as_ref()
        .map(|template| runtime_selection_topic(&template.strategy_id));
    let runtime_strategy_applier_failure = Arc::new(Mutex::new(None::<String>));
    let mut runtime_strategy_shutdown = None;
    let selector_poll_interval = Duration::from_millis(ruleset.selector_poll_interval_ms);
    let cancellation = CancellationToken::new();
    let audit_cfg = cfg
        .audit
        .as_ref()
        .context("platform runtime requires audit configuration")?;

    add_reference_actor(node, cfg)?;
    if let Some(runtime_strategy_template) = runtime_strategy_template {
        let active_runtime_strategy = register_runtime_strategy(
            node,
            &runtime_strategy_template,
            Arc::clone(&services.runtime_strategy_factory),
        )?;
        runtime_strategy_shutdown = Some(RuntimeStrategyShutdown {
            trader: Rc::clone(node.kernel().trader()),
            strategy_id: active_runtime_strategy.strategy_id,
            owned_by_runtime: active_runtime_strategy.owned_by_runtime,
        });
        add_runtime_strategy_applier_actor(
            node,
            runtime_strategy_template,
            active_runtime_strategy,
            runtime_selection_topic
                .clone()
                .expect("runtime selection topic should exist with a template"),
            Arc::clone(&runtime_strategy_applier_failure),
            node.handle(),
            cancellation.clone(),
        )?;
    }
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
    let selector_task = if runtime_selection_topic.is_some() {
        tokio::task::spawn_local(run_selector_task(
            ruleset,
            selector_poll_interval,
            services.candidate_loader,
            services.now_ms,
            audit_tx,
            runtime_selection_topic,
            cancellation.clone(),
            node.handle(),
        ))
    } else {
        tokio::spawn(run_selector_task(
            ruleset,
            selector_poll_interval,
            services.candidate_loader,
            services.now_ms,
            audit_tx,
            runtime_selection_topic,
            cancellation.clone(),
            node.handle(),
        ))
    };

    Ok(PlatformRuntimeGuards {
        cancellation,
        reference_snapshot_audit: Some(reference_snapshot_audit),
        runtime_strategy_shutdown,
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
    active_runtime_strategy: RuntimeManagedStrategy,
    selection_topic: String,
    failure: Arc<Mutex<Option<String>>>,
    node_handle: LiveNodeHandle,
    cancellation: CancellationToken,
) -> Result<()> {
    let actor = RuntimeStrategyApplier::new(
        RuntimeStrategyApplierConfig {
            base: DataActorConfig {
                actor_id: Some(ActorId::from(
                    format!("RUNTIME-STRATEGY-APPLIER-{}", UUID4::new()).as_str(),
                )),
                ..Default::default()
            },
            selection_topic,
        },
        template,
        Rc::clone(node.kernel().trader()),
        active_runtime_strategy,
        failure,
        cancellation,
        node_handle,
    );

    node.add_actor(actor)
        .context("failed to register runtime strategy applier actor")?;
    Ok(())
}

fn register_runtime_strategy(
    node: &mut LiveNode,
    template: &RuntimeStrategyTemplate,
    runtime_strategy_factory: RuntimeStrategyFactory,
) -> Result<RuntimeManagedStrategy> {
    let trader = Rc::clone(node.kernel().trader());

    if trader
        .borrow()
        .strategy_ids()
        .contains(&template.strategy_id)
    {
        return Ok(RuntimeManagedStrategy {
            strategy_id: template.strategy_id,
            owned_by_runtime: false,
        });
    }

    let strategy_id = runtime_strategy_factory(&trader, &template.kind, &template.raw_config)
        .context("failed building runtime-managed strategy from template")?;

    Ok(RuntimeManagedStrategy {
        strategy_id,
        owned_by_runtime: true,
    })
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

#[allow(clippy::too_many_arguments)]
async fn run_selector_task(
    ruleset: RulesetConfig,
    poll_interval: Duration,
    selector_loader: Arc<dyn CandidateMarketLoader>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    audit_tx: AuditSender,
    runtime_selection_topic: Option<String>,
    cancellation: CancellationToken,
    node_handle: LiveNodeHandle,
) -> Result<()> {
    run_selector_task_inner(
        ruleset,
        poll_interval,
        selector_loader,
        now_ms,
        audit_tx,
        runtime_selection_topic,
        cancellation,
        node_handle,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_selector_task_inner(
    ruleset: RulesetConfig,
    poll_interval: Duration,
    selector_loader: Arc<dyn CandidateMarketLoader>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    audit_tx: AuditSender,
    runtime_selection_topic: Option<String>,
    cancellation: CancellationToken,
    node_handle: LiveNodeHandle,
) -> Result<()> {
    if !wait_for_node_running(&node_handle, &cancellation).await {
        return Ok(());
    }

    let mut ticker = interval(poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_runtime_selection_decision = None;

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
        let ts_ms = now_ms();
        if let Err(error) = send_selector_evaluation(&audit_tx, &evaluation, ts_ms) {
            if cancellation.is_cancelled() {
                return Ok(());
            }

            return Err(fail_closed(
                &node_handle,
                &cancellation,
                error.context("selector audit send failed"),
            ));
        }

        if let Some(runtime_selection_topic) = runtime_selection_topic.as_ref()
            && last_runtime_selection_decision.as_ref() != Some(&evaluation.decision)
        {
            publish_runtime_selection_snapshot(runtime_selection_topic, &evaluation, ts_ms);
            last_runtime_selection_decision = Some(evaluation.decision.clone());
        }
    }
}

fn runtime_strategy_template(cfg: &Config) -> Result<Option<RuntimeStrategyTemplate>> {
    let strategy = match cfg.strategies.as_slice() {
        [] => return Ok(None),
        [strategy] => strategy,
        _ => {
            bail!(
                "platform runtime supports at most one runtime strategy template, got {}",
                cfg.strategies.len()
            );
        }
    };

    let strategy_id = strategy
        .config
        .as_table()
        .and_then(|table| table.get("strategy_id"))
        .and_then(Value::as_str)
        .context("runtime strategy template must include strategy_id")?;

    Ok(Some(RuntimeStrategyTemplate {
        kind: strategy.kind.clone(),
        strategy_id: StrategyId::from(strategy_id),
        raw_config: strategy.config.clone(),
    }))
}

pub fn registry_runtime_strategy_factory(
    registry: StrategyRegistry,
    build_context: StrategyBuildContext,
) -> RuntimeStrategyFactory {
    Arc::new(move |trader, kind, raw_config| {
        registry.register_strategy(kind, raw_config, &build_context, trader)
    })
}

pub fn runtime_selection_topic(strategy_id: &StrategyId) -> String {
    format!("platform.runtime.selection.{strategy_id}")
}

fn publish_runtime_selection_snapshot(
    runtime_selection_topic: &str,
    evaluation: &SelectionEvaluation,
    published_at_ms: u64,
) {
    let snapshot = RuntimeSelectionSnapshot {
        ruleset_id: evaluation.decision.ruleset_id.clone(),
        decision: evaluation.decision.clone(),
        eligible_candidates: evaluation.eligible_candidates.clone(),
        published_at_ms,
    };

    msgbus::publish_any(runtime_selection_topic.to_string().into(), &snapshot);
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
    ts_ms: u64,
) -> Result<()> {
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

struct ProductionCandidateMarketLoader {
    selector_state: Option<PolymarketSelectorState>,
    gamma_event_fetch_max_concurrent: usize,
}

impl CandidateMarketLoader for ProductionCandidateMarketLoader {
    fn load(&self, ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
        let selector_state = self.selector_state.clone();
        let gamma_event_fetch_max_concurrent = self.gamma_event_fetch_max_concurrent;
        Box::pin(async move {
            load_candidate_markets_for_ruleset(
                &ruleset,
                ruleset.candidate_load_timeout_secs,
                gamma_event_fetch_max_concurrent,
                selector_state,
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AuditConfig, Config, ExecClientEntry, ExecClientSecrets, LoggingConfig, NodeConfig,
        RawCaptureConfig, ReferenceConfig, RulesetConfig, RulesetVenueKind, StrategyEntry,
        StreamingCaptureConfig,
    };
    use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
    use nautilus_live::node::LiveNode;
    use nautilus_model::identifiers::TraderId;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn config_with_runtime_strategy(kind: &str) -> Config {
        Config {
            node: NodeConfig {
                name: "TEST-NODE".to_string(),
                trader_id: "BOLT-001".to_string(),
                environment: "Live".to_string(),
                load_state: false,
                save_state: false,
                timeout_connection_secs: 1,
                timeout_reconciliation_secs: 1,
                timeout_portfolio_secs: 1,
                timeout_disconnection_secs: 1,
                delay_post_stop_secs: 0,
                delay_shutdown_secs: 0,
            },
            logging: LoggingConfig {
                stdout_level: "Info".to_string(),
                file_level: "Debug".to_string(),
            },
            data_clients: Vec::new(),
            exec_clients: vec![ExecClientEntry {
                name: "TEST".to_string(),
                kind: "polymarket".to_string(),
                config: toml::toml! {
                    account_id = "POLYMARKET-001"
                    signature_type = 2
                    funder = "0xabc"
                }
                .into(),
                secrets: ExecClientSecrets {
                    region: "us-east-1".to_string(),
                    pk: Some("/pk".to_string()),
                    api_key: Some("/key".to_string()),
                    api_secret: Some("/secret".to_string()),
                    passphrase: Some("/pass".to_string()),
                },
            }],
            exec_engine: crate::config::ExecEngineConfig::default(),
            strategies: vec![StrategyEntry {
                kind: kind.to_string(),
                config: toml::toml! {
                    strategy_id = "STUB-RUNTIME-001"
                }
                .into(),
            }],
            raw_capture: RawCaptureConfig::default(),
            streaming: StreamingCaptureConfig::default(),
            reference: ReferenceConfig::default(),
            rulesets: Vec::new(),
            audit: None,
        }
    }

    #[test]
    fn runtime_strategy_template_preserves_strategy_kind() {
        let cfg = config_with_runtime_strategy("stub_runtime_strategy");

        let template = runtime_strategy_template(&cfg)
            .expect("runtime strategy template should build")
            .expect("runtime strategy template should exist");

        assert_eq!(template.kind, "stub_runtime_strategy");
        assert_eq!(template.strategy_id, StrategyId::from("STUB-RUNTIME-001"));
    }

    struct NoopAuditTaskFactory;

    impl PlatformAuditTaskFactory for NoopAuditTaskFactory {
        fn spawn(
            &self,
            _audit_rx: AuditReceiver,
            _audit_config: AuditSpoolConfig,
            cancellation: CancellationToken,
        ) -> JoinHandle<Result<()>> {
            tokio::spawn(async move {
                cancellation.cancelled().await;
                Ok(())
            })
        }
    }

    fn config_with_ruleset_and_zero_templates() -> Config {
        Config {
            node: NodeConfig {
                name: "TEST-NODE".to_string(),
                trader_id: "BOLT-001".to_string(),
                environment: "Live".to_string(),
                load_state: false,
                save_state: false,
                timeout_connection_secs: 1,
                timeout_reconciliation_secs: 1,
                timeout_portfolio_secs: 1,
                timeout_disconnection_secs: 1,
                delay_post_stop_secs: 0,
                delay_shutdown_secs: 0,
            },
            logging: LoggingConfig {
                stdout_level: "Info".to_string(),
                file_level: "Debug".to_string(),
            },
            data_clients: Vec::new(),
            exec_clients: Vec::new(),
            exec_engine: crate::config::ExecEngineConfig::default(),
            strategies: Vec::new(),
            raw_capture: RawCaptureConfig::default(),
            streaming: StreamingCaptureConfig::default(),
            reference: ReferenceConfig {
                publish_topic: "platform.reference.test".to_string(),
                ..Default::default()
            },
            rulesets: vec![RulesetConfig {
                id: "PRIMARY".to_string(),
                venue: RulesetVenueKind::Polymarket,
                selector: toml::toml! {
                    tag_slug = "bitcoin"
                }
                .into(),
                resolution_basis: "binance_btcusdt_1m".to_string(),
                min_time_to_expiry_secs: 60,
                max_time_to_expiry_secs: 900,
                min_liquidity_num: 1_000.0,
                require_accepting_orders: true,
                freeze_before_end_secs: 90,
                selector_poll_interval_ms: 25,
                candidate_load_timeout_secs: 7,
            }],
            audit: Some(AuditConfig {
                local_dir: "var/audit".to_string(),
                s3_uri: "s3://bucket/audit".to_string(),
                ship_interval_secs: 1,
                upload_attempt_timeout_secs: 1,
                roll_max_bytes: 1_048_576,
                roll_max_secs: 300,
                max_local_backlog_bytes: 4 * 1_048_576,
            }),
        }
    }

    fn build_empty_node() -> LiveNode {
        LiveNode::builder(TraderId::from("BOLT-001"), Environment::Live)
            .expect("builder should construct")
            .with_name("TEST-NODE")
            .with_logging(LoggerConfig::default())
            .with_timeout_connection(1)
            .with_timeout_disconnection_secs(1)
            .with_delay_post_stop_secs(0)
            .with_delay_shutdown_secs(0)
            .build()
            .expect("node should build")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn zero_template_ruleset_runtime_omits_strategy_shutdown_guard() {
        let cfg = config_with_ruleset_and_zero_templates();
        let services = PlatformRuntimeServices {
            candidate_loader: Arc::new(|_ruleset: RulesetConfig| async move { Ok(Vec::new()) }),
            audit_task_factory: Arc::new(NoopAuditTaskFactory),
            now_ms: Arc::new(|| 1_000),
            runtime_strategy_factory: Arc::new(|_trader, kind, _raw_config| {
                panic!("runtime strategy factory should not be called without a template: {kind}")
            }),
        };

        let mut node = build_empty_node();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services)
            .expect("zero-template ruleset runtime should wire");

        assert!(
            guards.runtime_strategy_shutdown.is_none(),
            "zero-template ruleset runtime should not install a shutdown guard"
        );
        assert!(
            guards.runtime_strategy_applier_failure.is_some(),
            "zero-template ruleset runtime still tracks applier failures for a consistent guard shape"
        );

        guards.shutdown().await.expect("shutdown should succeed");
    }
}
