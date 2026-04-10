use std::{
    any::Any,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use nautilus_common::{
    actor::DataActorConfig,
    msgbus::{self, ShareableMessageHandler},
};
use nautilus_core::UUID4;
use nautilus_live::node::{LiveNode, LiveNodeHandle, NodeState};
use nautilus_model::identifiers::{ActorId, ClientId, InstrumentId};
use tokio::{
    task::JoinHandle,
    time::{MissedTickBehavior, interval, sleep},
};
use tokio_util::sync::CancellationToken;

use crate::{
    clients::{self, ReferenceDataClientParts},
    config::{Config, ReferenceVenueEntry, ReferenceVenueKind, RulesetConfig},
    platform::{
        audit::{
            AuditReceiver, AuditRecord, AuditSender, AuditSpoolConfig, AwsCliUploader,
            ReferenceVenueSnapshot, SelectorState, VenueHealthState, spawn_audit_worker,
        },
        polymarket_catalog::load_candidate_markets_for_ruleset,
        reference::{ReferenceSnapshot, VenueHealth},
        reference_actor::{ReferenceActor, ReferenceActorConfig, ReferenceSubscription},
        ruleset::{CandidateMarket, SelectionDecision, SelectionState, select_market},
    },
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
    pub join_handles: Vec<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl PlatformRuntimeGuards {
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.cancellation.cancel();

        let mut first_error = None;
        if let Some(reference_snapshot_audit) = self.reference_snapshot_audit.take() {
            if let Err(error) = reference_snapshot_audit.unsubscribe() {
                first_error = Some(error);
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
    let selector_poll_interval = Duration::from_millis(ruleset.selector_poll_interval_ms);
    let audit_cfg = cfg
        .audit
        .as_ref()
        .context("platform runtime requires audit configuration")?;

    add_reference_actor(node, cfg)?;

    let cancellation = CancellationToken::new();
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
    let selector_task = tokio::spawn(run_selector_task(
        ruleset,
        selector_poll_interval,
        services.candidate_loader,
        services.now_ms,
        audit_tx,
        cancellation.clone(),
        node.handle(),
    ));

    Ok(PlatformRuntimeGuards {
        cancellation,
        reference_snapshot_audit: Some(reference_snapshot_audit),
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

async fn run_selector_task(
    ruleset: RulesetConfig,
    poll_interval: Duration,
    selector_loader: Arc<dyn CandidateMarketLoader>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    audit_tx: AuditSender,
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
            if let Ok(mut send_failure) = handler_send_failure.lock() {
                if send_failure.is_none() {
                    *send_failure = Some(error_message.clone());
                }
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
                    observed_price: venue.observed_price,
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
