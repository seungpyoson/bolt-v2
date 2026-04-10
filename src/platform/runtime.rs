use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use nautilus_common::actor::DataActorConfig;
use nautilus_core::UUID4;
use nautilus_live::node::{LiveNode, LiveNodeHandle};
use nautilus_model::identifiers::{ActorId, ClientId, InstrumentId};
use tokio::{
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};
use tokio_util::sync::CancellationToken;

use crate::{
    clients::{self, ReferenceDataClientParts},
    config::{Config, ReferenceVenueEntry, ReferenceVenueKind, RulesetConfig},
    platform::{
        audit::{
            AuditReceiver, AuditRecord, AuditSender, AuditSpoolConfig, AuditWorkerHandle,
            AwsCliUploader, SelectorState, spawn_audit_worker,
        },
        polymarket_catalog::load_candidate_markets_for_ruleset,
        reference_actor::{ReferenceActor, ReferenceActorConfig, ReferenceSubscription},
        ruleset::{CandidateMarket, SelectionDecision, SelectionState, select_market},
    },
};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

pub type CandidateMarketLoadFuture = BoxFuture<Result<Vec<CandidateMarket>>>;
pub type AuditTaskFactory = Arc<
    dyn Fn(AuditReceiver, AuditSpoolConfig, CancellationToken) -> JoinHandle<Result<()>>
        + Send
        + Sync,
>;

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
    pub selector_poll_interval: Duration,
    pub candidate_loader: Arc<dyn CandidateMarketLoader>,
    pub audit_task_factory: Arc<dyn PlatformAuditTaskFactory>,
    pub now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

#[derive(Clone)]
pub struct PlatformRuntimeHooks {
    pub selector_poll_interval: Duration,
    pub selector_loader: Arc<dyn Fn(RulesetConfig) -> CandidateMarketLoadFuture + Send + Sync>,
    pub audit_task_factory: AuditTaskFactory,
}

impl PlatformRuntimeServices {
    pub fn production() -> Self {
        Self {
            selector_poll_interval: Duration::from_secs(1),
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
    pub join_handles: Vec<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl PlatformRuntimeGuards {
    pub async fn shutdown(self) -> anyhow::Result<()> {
        self.cancellation.cancel();
        for handle in self.join_handles {
            handle.await??;
        }
        Ok(())
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

pub fn wire_platform_runtime_with_hooks(
    node: &mut LiveNode,
    cfg: &Config,
    hooks: PlatformRuntimeHooks,
) -> anyhow::Result<PlatformRuntimeGuards> {
    wire_platform_runtime_with_services(
        node,
        cfg,
        PlatformRuntimeServices {
            selector_poll_interval: hooks.selector_poll_interval,
            candidate_loader: Arc::new(HookCandidateMarketLoader {
                loader: hooks.selector_loader,
            }),
            audit_task_factory: Arc::new(HookAuditTaskFactory {
                factory: hooks.audit_task_factory,
            }),
            now_ms: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock should be after UNIX_EPOCH")
                    .as_millis() as u64
            }),
        },
    )
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
        .map(clone_ruleset_config)
        .context("platform runtime requires one active ruleset")?;
    let audit_cfg = cfg
        .audit
        .as_ref()
        .context("platform runtime requires audit configuration")?;

    add_reference_actor(node, cfg)?;

    let cancellation = CancellationToken::new();
    let (audit_tx, audit_rx) = crate::platform::audit::audit_channel();
    let audit_task = services.audit_task_factory.spawn(
        audit_rx,
        AuditSpoolConfig {
            spool_dir: audit_cfg.local_dir.clone().into(),
            s3_prefix: audit_cfg.s3_uri.clone(),
            node_name: cfg.node.name.clone(),
            run_id: node.instance_id().to_string(),
            ship_interval: Duration::from_secs(audit_cfg.ship_interval_secs),
            upload_attempt_timeout: Duration::from_secs(30),
            roll_max_bytes: audit_cfg.roll_max_bytes,
            roll_max_secs: audit_cfg.roll_max_secs,
            max_local_backlog_bytes: audit_cfg.max_local_backlog_bytes,
        },
        cancellation.clone(),
    );
    let audit_task = tokio::spawn(supervise_audit_task(
        audit_task,
        cancellation.clone(),
        node.handle(),
    ));

    let selector_task = tokio::spawn(run_selector_task(
        ruleset,
        services.selector_poll_interval,
        services.candidate_loader,
        services.now_ms,
        audit_tx,
        cancellation.clone(),
        node.handle(),
    ));

    Ok(PlatformRuntimeGuards {
        cancellation,
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
    let venue_cfgs = cfg
        .reference
        .venues
        .iter()
        .map(clone_reference_venue_entry)
        .collect();

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
        ReferenceVenueKind::Polymarket => cfg
            .data_clients
            .iter()
            .find(|client| client.kind == "polymarket")
            .map(|client| client.name.clone())
            .context("reference polymarket venue requires the primary polymarket data client"),
        ReferenceVenueKind::Chainlink => bail!("chainlink reference client is not implemented yet"),
    }
}

fn spawn_audit_shutdown_task(
    worker: AuditWorkerHandle,
    cancellation: CancellationToken,
) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        cancellation.cancelled().await;
        worker.shutdown().await
    })
}

async fn run_selector_task(
    ruleset: RulesetConfig,
    poll_interval: Duration,
    selector_loader: Arc<dyn CandidateMarketLoader>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    audit_tx: AuditSender,
    cancellation: CancellationToken,
    node_handle: LiveNodeHandle,
) -> Result<()> {
    let mut ticker = interval(poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        if !node_handle.is_running() {
            tokio::select! {
                _ = cancellation.cancelled() => return Ok(()),
                _ = tokio::time::sleep(Duration::from_millis(10)) => continue,
            }
        }

        tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            _ = ticker.tick() => {}
        }

        let load_future = selector_loader.load(clone_ruleset_config(&ruleset));
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

        let decision = select_market(&ruleset, &candidates);
        if let Err(error) = send_selector_decision(&audit_tx, &decision, &now_ms) {
            if cancellation.is_cancelled() {
                return Ok(());
            }

            return Err(fail_closed(
                &node_handle,
                &cancellation,
                error.context("selector audit send failed"),
            ));
        }
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

fn send_selector_decision(
    audit_tx: &AuditSender,
    decision: &SelectionDecision,
    now_ms: &Arc<dyn Fn() -> u64 + Send + Sync>,
) -> Result<()> {
    let ts_ms = now_ms();
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

fn clone_reference_venue_entry(venue: &ReferenceVenueEntry) -> ReferenceVenueEntry {
    ReferenceVenueEntry {
        name: venue.name.clone(),
        kind: venue.kind.clone(),
        instrument_id: venue.instrument_id.clone(),
        base_weight: venue.base_weight,
        stale_after_ms: venue.stale_after_ms,
        disable_after_ms: venue.disable_after_ms,
    }
}

fn clone_ruleset_config(ruleset: &RulesetConfig) -> RulesetConfig {
    RulesetConfig {
        id: ruleset.id.clone(),
        venue: ruleset.venue.clone(),
        tag_slug: ruleset.tag_slug.clone(),
        resolution_basis: ruleset.resolution_basis.clone(),
        min_time_to_expiry_secs: ruleset.min_time_to_expiry_secs,
        max_time_to_expiry_secs: ruleset.max_time_to_expiry_secs,
        min_liquidity_num: ruleset.min_liquidity_num,
        require_accepting_orders: ruleset.require_accepting_orders,
        freeze_before_end_secs: ruleset.freeze_before_end_secs,
    }
}

struct ProductionCandidateMarketLoader;

impl CandidateMarketLoader for ProductionCandidateMarketLoader {
    fn load(&self, ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
        Box::pin(async move {
            load_candidate_markets_for_ruleset(&ruleset, 30)
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
        spawn_audit_shutdown_task(worker, cancellation)
    }
}

struct HookCandidateMarketLoader {
    loader: Arc<dyn Fn(RulesetConfig) -> CandidateMarketLoadFuture + Send + Sync>,
}

impl CandidateMarketLoader for HookCandidateMarketLoader {
    fn load(&self, ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
        (self.loader)(ruleset)
    }
}

struct HookAuditTaskFactory {
    factory: AuditTaskFactory,
}

impl PlatformAuditTaskFactory for HookAuditTaskFactory {
    fn spawn(
        &self,
        audit_rx: AuditReceiver,
        audit_config: AuditSpoolConfig,
        cancellation: CancellationToken,
    ) -> JoinHandle<Result<()>> {
        (self.factory)(audit_rx, audit_config, cancellation)
    }
}
