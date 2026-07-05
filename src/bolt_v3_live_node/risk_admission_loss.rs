use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use nautilus_common::{
    factories::OrderFactory,
    messages::execution::{SubmitOrder, TradingCommand},
};
use nautilus_model::events::OrderEventAny;
use tokio::sync::Notify;

use crate::bolt_v3_operator_health::BoltV3OperatorHealthTransitionEmitter;
use crate::bolt_v3_venue_truth::{
    VenueTruthCaptureFailureEvidence, venue_truth_capture_failure_parts,
};
use crate::{
    bolt_v3_config::{KillSwitchFlattenConfigBlock, KillSwitchFlattenRouteKindConfig},
    bolt_v3_kill_switch_flatten::{
        BoltV3KillSwitchFlattenCandidate, BoltV3KillSwitchFlattenCommand,
        BoltV3KillSwitchFlattenPlan, BoltV3KillSwitchFlattenPlanRequest,
        BoltV3KillSwitchFlattenPolicy, BoltV3KillSwitchFlattenPositionEvidenceKind,
        BoltV3KillSwitchFlattenPositionState, BoltV3KillSwitchFlattenRouteKind,
        BoltV3KillSwitchFlattenRouteProof, BoltV3KillSwitchFlattenSnapshot,
        BoltV3KillSwitchFlattenSupervisor,
    },
    bolt_v3_order_execution::{
        BoltV3KillSwitchFlattenRoutingContext, BoltV3NtSubmitOnlySink, BoltV3OrderExecutionPolicy,
        route_kill_switch_flatten_command_with_sink,
    },
    bolt_v3_order_intent::NtOrderTemplate,
    bolt_v3_submit_admission::{
        BoltV3KillSwitchForcedReductionClaim, BoltV3KillSwitchForcedReductionPolicy,
        BoltV3SubmitLifecyclePolicy,
    },
};

use super::*;

const OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_FAILURE: &str =
    stringify!(venue_truth_capture_failure);
const OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_RECOVERY: &str =
    stringify!(venue_truth_capture_recovery);
const OPERATOR_HEALTH_REASON_VENUE_TRUTH_RUNTIME_FAILURE: &str =
    stringify!(venue_truth_runtime_failure);
const OPERATOR_HEALTH_REASON_VENUE_TRUTH_DIVERGENCE: &str = stringify!(venue_truth_divergence);

struct ClosureKillSwitchFlattenExecutor<F: Fn(&KillSwitchLossAction) -> Result<()>> {
    execute_flatten: F,
}

impl<F> KillSwitchFlattenExecutor for ClosureKillSwitchFlattenExecutor<F>
where
    F: Fn(&KillSwitchLossAction) -> Result<()>,
{
    fn execute_flatten(&self, action: &KillSwitchLossAction) -> Result<()> {
        (self.execute_flatten)(action)
    }
}

#[derive(Debug, Clone)]
pub(super) struct BoltV3CapitalAdmissionVenueSpendabilitySourceConfig {
    pub(super) path: PathBuf,
    pub(super) max_bytes: u64,
    pub(super) expected_sha256: String,
    pub(super) venue_id: String,
    pub(super) account_id: String,
    pub(super) collateral_currency: String,
}

/// Startup reservation-recovery source: the decision-evidence file the
/// live-node boot driver reads to recover known submit-reservation
/// metadata after a restart, plus the byte cap from
/// [`crate::bolt_v3_config::DecisionEvidenceBlock::recovery_evidence_max_bytes`].
#[derive(Debug, Clone)]
pub(super) struct BoltV3SubmitReservationRecoveryConfig {
    pub(super) path: PathBuf,
    pub(super) max_bytes: u64,
}

pub(super) struct BoltV3VenueTruthRuntimeConfig {
    pub(super) source: Arc<dyn crate::bolt_v3_venue_truth::VenueTruthSnapshotSource>,
    pub(super) order_event_mapper: Arc<dyn crate::bolt_v3_venue_truth::VenueTruthOrderEventMapper>,
    pub(super) poll_interval_ms: u64,
    pub(super) kill_switch_store: KillSwitchStore,
}

pub(super) struct BoltV3VenueTruthRuntimeGuard {
    shutdown_requested: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl BoltV3VenueTruthRuntimeGuard {
    pub(super) fn stop_and_join(mut self) {
        self.stop_and_join_inner();
    }

    fn stop_and_join_inner(&mut self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        self.shutdown_notify.notify_waiters();
        if let Some(handle) = self.handle.take()
            && let Err(error) = handle.join()
        {
            log::error!("venue truth runtime thread join failed: {error:?}");
        }
    }
}

impl Drop for BoltV3VenueTruthRuntimeGuard {
    fn drop(&mut self) {
        self.stop_and_join_inner();
    }
}

pub(super) fn loss_governor_runtime_feed_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LossGovernorRuntimeFeedConfig>, BoltV3LiveNodeError> {
    let Some(block) = loaded.root.risk.loss_governor.as_ref() else {
        return Ok(None);
    };
    if !block.enabled {
        return Ok(None);
    }
    Ok(Some(LossGovernorRuntimeFeedConfig {
        account_id: block.account_id,
        rolling_window_ns: block.rolling_window_ns,
        active_position_pnl_max_entries: required_loss_governor_usize(
            "risk.loss_governor.active_position_pnl_max_entries",
            block.active_position_pnl_max_entries,
        )?,
    }))
}

pub(super) fn venue_truth_runtime_config_from_loaded(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    feed_config: Option<&CapitalAdmissionRuntimeFeedConfig>,
) -> Result<Option<BoltV3VenueTruthRuntimeConfig>, BoltV3LiveNodeError> {
    let Some(feed_config) = feed_config else {
        return Ok(None);
    };
    let Some(binding) = crate::bolt_v3_providers::binding_for_provider_key(&feed_config.venue_id)
    else {
        return Ok(None);
    };
    let Some(build_source) = binding.build_venue_truth_runtime_source else {
        return Ok(None);
    };
    let matching_clients = loaded
        .root
        .clients
        .iter()
        .filter(|(_, client)| {
            client.venue.as_str() == feed_config.venue_id && client.execution.is_some()
        })
        .collect::<Vec<_>>();
    let (client_key, client) = match matching_clients.as_slice() {
        [] => {
            return Err(BoltV3LiveNodeError::Build(anyhow::anyhow!(
                "capital admission requires a configured execution client for venue truth on venue `{}`",
                feed_config.venue_id
            )));
        }
        [(client_key, client)] => (client_key.as_str(), *client),
        _ => {
            return Err(BoltV3LiveNodeError::Build(anyhow::anyhow!(
                "capital admission requires one execution client for venue truth on venue `{}`; found {}",
                feed_config.venue_id,
                matching_clients
                    .iter()
                    .map(|(client_key, _)| client_key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    };
    let source = build_source(crate::bolt_v3_providers::ProviderVenueTruthSourceContext {
        client_key,
        client,
        resolved,
        collateral_currency: feed_config.collateral_currency.as_str(),
    })
    .map_err(BoltV3LiveNodeError::Build)?;

    Ok(Some(BoltV3VenueTruthRuntimeConfig {
        source: source.source,
        order_event_mapper: source.order_event_mapper,
        poll_interval_ms: source.poll_interval_ms,
        kill_switch_store: venue_truth_kill_switch_store_from_loaded(loaded)?,
    }))
}

fn venue_truth_kill_switch_store_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<KillSwitchStore, BoltV3LiveNodeError> {
    let Some(kill_switch) = loaded
        .root
        .risk
        .kill_switch
        .as_ref()
        .filter(|kill_switch| kill_switch.enabled)
    else {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "risk.kill_switch.enabled=true is required when venue truth is enforced"
        )));
    };
    Ok(KillSwitchStore::from_root_config_path(
        &loaded.root_path,
        kill_switch,
    ))
}

pub(super) fn spawn_venue_truth_runtime(
    config: BoltV3VenueTruthRuntimeConfig,
    feed: Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    stop_handle: LiveNodeHandle,
    health_emitter: Option<BoltV3OperatorHealthTransitionEmitter>,
) -> BoltV3VenueTruthRuntimeGuard {
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let shutdown_notify = Arc::new(Notify::new());
    let thread_shutdown_requested = Arc::clone(&shutdown_requested);
    let thread_shutdown_notify = Arc::clone(&shutdown_notify);
    let spawn_submit_admission = Arc::clone(&submit_admission);
    let spawn_stop_handle = stop_handle.clone();
    let spawn_health_emitter = health_emitter.clone();
    let handle = std::thread::Builder::new()
        .name("bolt-v3-venue-truth-runtime".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    halt_for_venue_truth(
                        &spawn_submit_admission,
                        &spawn_stop_handle,
                        0,
                        format!("venue truth runtime build failed: {error:#}"),
                        spawn_health_emitter.as_ref(),
                    );
                    return;
                }
            };
            runtime.block_on(run_venue_truth_runtime(
                config,
                feed,
                spawn_submit_admission,
                spawn_stop_handle,
                thread_shutdown_requested,
                thread_shutdown_notify,
                spawn_health_emitter,
            ));
        });
    let handle = match handle {
        Ok(handle) => Some(handle),
        Err(error) => {
            halt_for_venue_truth(
                &submit_admission,
                &stop_handle,
                0,
                format!("venue truth runtime thread spawn failed: {error:#}"),
                health_emitter.as_ref(),
            );
            None
        }
    };
    BoltV3VenueTruthRuntimeGuard {
        shutdown_requested,
        shutdown_notify,
        handle,
    }
}

async fn run_venue_truth_runtime(
    config: BoltV3VenueTruthRuntimeConfig,
    feed: Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    stop_handle: LiveNodeHandle,
    shutdown_requested: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
    health_emitter: Option<BoltV3OperatorHealthTransitionEmitter>,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(config.poll_interval_ms));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut captures_missed = 0_u64;
    loop {
        if shutdown_requested.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            () = shutdown_notify.notified() => break,
            _ = interval.tick() => {}
        }
        if shutdown_requested.load(Ordering::SeqCst) {
            break;
        }
        let captured_at = match current_unix_nanos() {
            Ok(value) => value,
            Err(error) => {
                halt_for_venue_truth(
                    &submit_admission,
                    &stop_handle,
                    0,
                    format!("clock failed before venue truth poll: {error:#}"),
                    health_emitter.as_ref(),
                );
                break;
            }
        };
        let snapshot = tokio::select! {
            () = shutdown_notify.notified() => break,
            result = config.source.snapshot(nautilus_core::UnixNanos::from(captured_at)) => result,
        };
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                captures_missed = captures_missed.saturating_add(1);
                handle_venue_truth_capture_failure(
                    &submit_admission,
                    captured_at,
                    captures_missed,
                    &error,
                );
                if let Some(health_emitter) = health_emitter.as_ref() {
                    health_emitter(OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_FAILURE);
                }
                continue;
            }
        };
        captures_missed = 0;
        let reconcile = reconcile_venue_truth_snapshot_with_health_emission(
            &feed,
            snapshot,
            health_emitter.as_ref(),
        );
        if let Err(divergence) = reconcile {
            halt_for_venue_truth_divergence(
                &submit_admission,
                &config.kill_switch_store,
                &stop_handle,
                *divergence,
                health_emitter.as_ref(),
            );
            break;
        }
    }
}

fn reconcile_venue_truth_snapshot_with_health_emission(
    feed: &Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
    snapshot: crate::bolt_v3_venue_truth::VenueTruthSnapshot,
    health_emitter: Option<&BoltV3OperatorHealthTransitionEmitter>,
) -> Result<
    Option<BoltV3SubmitCapitalAdmissionNtComponents>,
    Box<crate::bolt_v3_venue_truth::VenueTruthDivergence>,
> {
    let reconcile = reconcile_venue_truth_snapshot(feed, snapshot)?;
    if let Some(health_emitter) = health_emitter {
        health_emitter(OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_RECOVERY);
    }
    Ok(reconcile)
}

fn reconcile_venue_truth_snapshot(
    feed: &Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
    snapshot: crate::bolt_v3_venue_truth::VenueTruthSnapshot,
) -> Result<
    Option<BoltV3SubmitCapitalAdmissionNtComponents>,
    Box<crate::bolt_v3_venue_truth::VenueTruthDivergence>,
> {
    let mut feed = feed
        .lock()
        .expect("venue truth reconcile feed lock poisoned");
    feed.on_venue_truth_snapshot(snapshot)
}

fn handle_venue_truth_capture_failure(
    submit_admission: &BoltV3SubmitAdmissionState,
    observed_at_ns: u64,
    captures_missed: u64,
    error: &anyhow::Error,
) {
    log::error!("venue truth poll failed: {error:#}");
    submit_admission.suspend_capital_admission_for_venue_truth_capture_failure(
        venue_truth_capture_failure_evidence(observed_at_ns, captures_missed, error),
    );
}

fn venue_truth_capture_failure_evidence(
    observed_at_ns: u64,
    captures_missed: u64,
    error: &anyhow::Error,
) -> VenueTruthCaptureFailureEvidence {
    let (endpoint, error_class) = venue_truth_capture_failure_parts(error);
    VenueTruthCaptureFailureEvidence {
        source: crate::bolt_v3_capital_admission_runtime_feed::POLYMARKET_VENUE_TRUTH_REST_SOURCE
            .to_string(),
        observed_at_ns,
        endpoint: endpoint.to_string(),
        error_class: error_class.to_string(),
        captures_missed,
    }
}

fn halt_for_venue_truth(
    submit_admission: &BoltV3SubmitAdmissionState,
    stop_handle: &LiveNodeHandle,
    source_timestamp_unix_nanos: u64,
    reason: String,
    health_emitter: Option<&BoltV3OperatorHealthTransitionEmitter>,
) {
    let state = latch_non_durable_venue_truth_runtime_failure(
        submit_admission,
        source_timestamp_unix_nanos,
        reason,
    );
    log::error!(
        "venue truth runtime failure latched memory-only kill switch: {:?}",
        state.kind()
    );
    if let Some(health_emitter) = health_emitter {
        health_emitter(OPERATOR_HEALTH_REASON_VENUE_TRUTH_RUNTIME_FAILURE);
    }
    stop_handle.stop();
}

fn halt_for_venue_truth_divergence(
    submit_admission: &BoltV3SubmitAdmissionState,
    kill_switch_store: &KillSwitchStore,
    stop_handle: &LiveNodeHandle,
    divergence: crate::bolt_v3_venue_truth::VenueTruthDivergence,
    health_emitter: Option<&BoltV3OperatorHealthTransitionEmitter>,
) {
    let state =
        durably_halt_for_venue_truth_divergence(submit_admission, kill_switch_store, divergence);
    log::error!(
        "venue truth divergence latched kill switch: {:?}",
        state.kind()
    );
    if let Some(health_emitter) = health_emitter {
        health_emitter(OPERATOR_HEALTH_REASON_VENUE_TRUTH_DIVERGENCE);
    }
    stop_handle.stop();
}

fn durably_halt_for_venue_truth_divergence(
    submit_admission: &BoltV3SubmitAdmissionState,
    kill_switch_store: &KillSwitchStore,
    divergence: crate::bolt_v3_venue_truth::VenueTruthDivergence,
) -> KillSwitchState {
    let source = crate::bolt_v3_capital_admission_runtime_feed::POLYMARKET_VENUE_TRUTH_REST_SOURCE;
    let reason = format!(
        "venue truth divergence: {:?} alarm_class={:?}",
        divergence.kind, divergence.alarm_class
    );
    let trigger = KillSwitchHaltTrigger::venue_truth_divergence(
        source,
        divergence.current_captured_at.as_u64(),
        reason,
    );
    let evidence = divergence.evidence(source);
    let evidence_write_error = submit_admission
        .record_venue_truth_divergence_evidence(&evidence)
        .err()
        .map(|error| {
            log::error!("failed to record venue truth divergence evidence: {error:#}");
            format!("venue truth divergence evidence write failed: {error:#}")
        });
    durably_halt_for_venue_truth_trigger(
        submit_admission,
        kill_switch_store,
        trigger,
        evidence_write_error,
    )
}

fn latch_non_durable_venue_truth_runtime_failure(
    submit_admission: &BoltV3SubmitAdmissionState,
    source_timestamp_unix_nanos: u64,
    reason: String,
) -> KillSwitchState {
    let current = submit_admission.kill_switch_state();
    if current.kind() != KillSwitchStateKind::Armed {
        return current;
    }
    let source = crate::bolt_v3_capital_admission_runtime_feed::POLYMARKET_VENUE_TRUTH_REST_SOURCE;
    let trigger = KillSwitchHaltTrigger::venue_truth_divergence(
        source,
        source_timestamp_unix_nanos,
        reason.clone(),
    );
    let fallback_halt_id = crate::bolt_v3_kill_switch::halt_id_for_trigger(&trigger);
    let failed = transition_kill_switch_state(
        KillSwitchState::Armed,
        KillSwitchEvent::HaltTriggered(trigger),
        venue_truth_kill_switch_transition_context(false, false),
    )
    .and_then(|halting| {
        transition_kill_switch_state(
            halting,
            KillSwitchEvent::HaltActionDispatchFailed { reason },
            venue_truth_kill_switch_transition_context(false, false),
        )
    })
    .unwrap_or_else(|error| KillSwitchState::FailedManualIntervention {
        halt_id: fallback_halt_id,
        reason: format!("venue truth runtime fail-closed transition failed: {error:?}"),
    });
    submit_admission.replace_kill_switch_state(failed.clone());
    failed
}

fn durably_halt_for_venue_truth_trigger(
    submit_admission: &BoltV3SubmitAdmissionState,
    kill_switch_store: &KillSwitchStore,
    trigger: KillSwitchHaltTrigger,
    evidence_write_error: Option<String>,
) -> KillSwitchState {
    let current = match kill_switch_store.load_recovery_state() {
        Ok(KillSwitchRecoveryState::Recovered(state))
        | Ok(KillSwitchRecoveryState::FailClosed {
            state: Some(state), ..
        }) => state,
        Ok(KillSwitchRecoveryState::FailClosed {
            reason,
            state: None,
        }) => {
            let failed = venue_truth_failed_manual_intervention_state(
                &trigger,
                format!("kill switch recovery failed without state: {reason:?}"),
            );
            return persist_venue_truth_failed_state(
                submit_admission,
                kill_switch_store,
                failed,
                "kill switch recovery failed without state",
            );
        }
        Err(error) => {
            let failed = venue_truth_failed_manual_intervention_state(
                &trigger,
                format!("kill switch recovery load failed: {error:?}"),
            );
            return persist_venue_truth_failed_state(
                submit_admission,
                kill_switch_store,
                failed,
                "kill switch recovery load failed",
            );
        }
    };
    if current.kind() != KillSwitchStateKind::Armed {
        submit_admission.replace_kill_switch_state(current.clone());
        return current;
    }

    let halting = match transition_kill_switch_state(
        current,
        KillSwitchEvent::HaltTriggered(trigger.clone()),
        venue_truth_kill_switch_transition_context(false, false),
    ) {
        Ok(state) => state,
        Err(error) => {
            let failed = venue_truth_failed_manual_intervention_state(
                &trigger,
                format!("venue truth halt transition failed: {error:?}"),
            );
            return persist_venue_truth_failed_state(
                submit_admission,
                kill_switch_store,
                failed,
                "venue truth halt transition failed",
            );
        }
    };

    if let Some(error) = evidence_write_error {
        let failed = venue_truth_failed_from_halting(halting, error);
        return persist_venue_truth_failed_state(
            submit_admission,
            kill_switch_store,
            failed,
            "venue truth evidence write failed",
        );
    }

    if let Err(error) = kill_switch_store.write_state(&halting) {
        let failed = venue_truth_failed_from_halting(
            halting,
            format!("kill switch state write failed: {error:?}"),
        );
        return persist_venue_truth_failed_state(
            submit_admission,
            kill_switch_store,
            failed,
            "kill switch halting state write failed",
        );
    }

    let halted = match transition_kill_switch_state(
        halting,
        KillSwitchEvent::DurableHaltEvidenceRecorded,
        venue_truth_kill_switch_transition_context(true, true),
    ) {
        Ok(state) => state,
        Err(error) => {
            let failed = venue_truth_failed_manual_intervention_state(
                &trigger,
                format!("venue truth halt transition failed: {error:?}"),
            );
            return persist_venue_truth_failed_state(
                submit_admission,
                kill_switch_store,
                failed,
                "venue truth halted transition failed",
            );
        }
    };
    if let Err(error) = kill_switch_store.write_state(&halted) {
        let KillSwitchState::Halted { halt_id, .. } = halted else {
            unreachable!();
        };
        let failed = KillSwitchState::FailedManualIntervention {
            halt_id,
            reason: format!("kill switch state write failed: {error:?}"),
        };
        return persist_venue_truth_failed_state(
            submit_admission,
            kill_switch_store,
            failed,
            "kill switch halted state write failed",
        );
    }
    submit_admission.replace_kill_switch_state(halted.clone());
    halted
}

fn persist_venue_truth_failed_state(
    submit_admission: &BoltV3SubmitAdmissionState,
    kill_switch_store: &KillSwitchStore,
    failed: KillSwitchState,
    context: &str,
) -> KillSwitchState {
    let state = match kill_switch_store.write_state(&failed) {
        Ok(()) => failed,
        Err(error) => {
            log::error!(
                "failed to persist venue truth fail-closed kill switch state after {context}: {error:#}"
            );
            venue_truth_failed_state_with_persist_error(failed, context, &error)
        }
    };
    submit_admission.replace_kill_switch_state(state.clone());
    state
}

fn venue_truth_failed_state_with_persist_error(
    state: KillSwitchState,
    context: &str,
    error: &impl std::fmt::Debug,
) -> KillSwitchState {
    match state {
        KillSwitchState::FailedManualIntervention { halt_id, reason } => {
            KillSwitchState::FailedManualIntervention {
                halt_id,
                reason: format!(
                    "{reason}; fail-closed kill switch state write failed after {context}: {error:?}"
                ),
            }
        }
        other => other,
    }
}

fn venue_truth_failed_from_halting(state: KillSwitchState, reason: String) -> KillSwitchState {
    let halt_id = match &state {
        KillSwitchState::Halting { halt_id, .. } => halt_id.clone(),
        _ => unreachable!("venue truth fail-closed transition requires halting state"),
    };
    match transition_kill_switch_state(
        state,
        KillSwitchEvent::DurableHaltEvidenceWriteFailed { reason },
        venue_truth_kill_switch_transition_context(false, false),
    ) {
        Ok(state) => state,
        Err(error) => KillSwitchState::FailedManualIntervention {
            halt_id,
            reason: format!("venue truth fail-closed transition failed: {error:?}"),
        },
    }
}

fn venue_truth_failed_manual_intervention_state(
    trigger: &KillSwitchHaltTrigger,
    reason: String,
) -> KillSwitchState {
    KillSwitchState::FailedManualIntervention {
        halt_id: crate::bolt_v3_kill_switch::halt_id_for_trigger(trigger),
        reason,
    }
}

fn venue_truth_kill_switch_transition_context(
    state_write_succeeded: bool,
    durable_halt_evidence_recorded: bool,
) -> KillSwitchTransitionContext {
    KillSwitchTransitionContext {
        state_write_succeeded,
        durable_halt_evidence_recorded,
        operator_authorized: false,
        manual_reset_evidence_valid: false,
        mandatory_proof_streams_fresh: false,
        no_outstanding_order_risk: false,
        no_open_positions: false,
        no_pending_entry_risk: false,
    }
}

pub(super) fn capital_admission_runtime_feed_config_from_loaded(
    loaded: &LoadedBoltV3Config,
    startup_observed_at_ns: u64,
) -> Option<CapitalAdmissionRuntimeFeedConfig> {
    let pools = loaded.root.risk.capital_pools.as_ref()?;
    let pool = pools.iter().find(|pool| pool.enforce_submit_admission)?;
    let product = pool.prediction_market_binary.as_ref()?;
    Some(CapitalAdmissionRuntimeFeedConfig {
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id,
        collateral_currency: pool.collateral_currency.clone(),
        product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
            PredictionMarketAdmissionSnapshot {
                source: "bolt_configured_binary_product".to_string(),
                observed_at_ns: startup_observed_at_ns,
                yes_instrument_id: product.yes_instrument_id.to_string(),
                no_instrument_id: product.no_instrument_id.to_string(),
                yes_position: Decimal::ZERO,
                no_position: Decimal::ZERO,
                collateral_allowance: Decimal::ZERO,
                conditional_token_allowance: Decimal::ZERO,
                collateral_coupled_group_id: product.collateral_coupled_group_id.clone(),
            },
        ),
        startup_observed_at_ns,
        dedupe_retention_ns: pool.dedupe_retention_ns,
    })
}

pub(super) fn order_reject_observer_account_id_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Option<AccountId> {
    let pools = loaded.root.risk.capital_pools.as_ref()?;
    let pool = pools.iter().find(|pool| pool.enforce_submit_admission)?;
    Some(pool.account_id)
}

pub(super) fn capital_admission_venue_spendability_source_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3CapitalAdmissionVenueSpendabilitySourceConfig>, BoltV3LiveNodeError> {
    let Some(pool) = loaded
        .root
        .risk
        .capital_pools
        .as_ref()
        .and_then(|pools| pools.iter().find(|pool| pool.enforce_submit_admission))
    else {
        return Ok(None);
    };
    let has_source_binding = pool.venue_spendability_source_path.is_some()
        || pool.venue_spendability_source_sha256.is_some()
        || pool.venue_spendability_source_max_bytes.is_some();
    if !has_source_binding {
        return Ok(None);
    }
    let (Some(path_value), Some(expected_sha256), Some(max_bytes)) = (
        pool.venue_spendability_source_path.as_ref(),
        pool.venue_spendability_source_sha256.as_ref(),
        pool.venue_spendability_source_max_bytes,
    ) else {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "risk.capital_pools venue_spendability_source path, sha256, and max_bytes must be configured together"
        )));
    };
    Ok(Some(BoltV3CapitalAdmissionVenueSpendabilitySourceConfig {
        path: resolve_root_relative_path(&loaded.root_path, path_value),
        max_bytes,
        expected_sha256: expected_sha256.clone(),
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id.to_string(),
        collateral_currency: pool.collateral_currency.clone(),
    }))
}

/// Resolve the startup reservation-recovery source from the loaded config.
/// The recovery driver reads the decision-evidence file, so the path comes
/// from [`decision_evidence_path`] and the read bound from
/// `persistence.decision_evidence.recovery_evidence_max_bytes`. Returns
/// `None` (recovery disabled) when the byte cap is not configured.
pub(super) fn submit_reservation_recovery_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3SubmitReservationRecoveryConfig>, BoltV3LiveNodeError> {
    let Some(max_bytes) = loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes
    else {
        return Ok(None);
    };
    Ok(Some(BoltV3SubmitReservationRecoveryConfig {
        path: decision_evidence_path(loaded).map_err(BoltV3LiveNodeError::Build)?,
        max_bytes,
    }))
}

pub(super) fn capital_admission_venue_spendability_snapshot_from_source_config(
    config: &BoltV3CapitalAdmissionVenueSpendabilitySourceConfig,
) -> Result<VenueSpendabilitySnapshot, BoltV3LiveNodeError> {
    venue_spendability_snapshot_from_json_file(VenueSpendabilitySourceFileRequest {
        path: &config.path,
        max_bytes: config.max_bytes,
        expected_sha256: &config.expected_sha256,
        identity: VenueSpendabilityIdentity {
            venue_id: &config.venue_id,
            account_id: &config.account_id,
            collateral_currency: &config.collateral_currency,
        },
    })
    .map_err(|error| {
        BoltV3LiveNodeError::Build(anyhow::anyhow!(
            "capital admission venue spendability source rejected: {error:?}"
        ))
    })
}

pub(super) fn refresh_capital_admission_venue_spendability_from_source(
    feed: &Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
    config: &BoltV3CapitalAdmissionVenueSpendabilitySourceConfig,
) -> Result<Option<BoltV3SubmitCapitalAdmissionNtComponents>, BoltV3LiveNodeError> {
    let snapshot = capital_admission_venue_spendability_snapshot_from_source_config(config)?;
    let mut feed = feed
        .lock()
        .expect("capital admission venue spendability feed lock poisoned");
    Ok(feed.on_venue_spendability_snapshot(snapshot))
}

pub(super) fn capital_admission_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3SubmitCapitalAdmissionConfig>, BoltV3LiveNodeError> {
    let Some(pools) = loaded.root.risk.capital_pools.as_ref() else {
        return Ok(None);
    };
    let Some(pool) = pools.iter().find(|pool| pool.enforce_submit_admission) else {
        return Ok(None);
    };
    Ok(Some(BoltV3SubmitCapitalAdmissionConfig {
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id.to_string(),
        product_kind: ProductKind::PredictionMarketBinary,
        collateral_currency: pool.collateral_currency.clone(),
        capital_pool: CapitalPoolSnapshot {
            source: pool.pool_id.clone(),
            observed_at_ns: 0,
            pool_id: pool.pool_id.clone(),
            max_pool_liability: required_pool_decimal(
                "risk.capital_pools.max_pool_liability",
                &pool.max_pool_liability,
            )?,
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: pool.max_snapshot_age_ns,
        },
        policy: capital_admission_policy_from_pool(pool)?,
        dedupe_retention_ns: pool.dedupe_retention_ns,
    }))
}

fn capital_admission_policy_from_pool(
    pool: &CapitalPoolBlock,
) -> Result<CapitalAdmissionPolicy, BoltV3LiveNodeError> {
    let policy = &pool.capital_admission_policy;
    Ok(CapitalAdmissionPolicy {
        min_remaining_pool_balance: optional_pool_decimal(
            "risk.capital_pools.capital_admission_policy.min_remaining_pool_balance",
            policy.min_remaining_pool_balance.as_deref(),
        )?,
        fee_slippage_policy: Some(FeeSlippagePolicy {
            max_fee_liability: required_pool_decimal(
                "risk.capital_pools.capital_admission_policy.fee_slippage.max_fee_liability",
                &policy.fee_slippage.max_fee_liability,
            )?,
            max_slippage_liability: required_pool_decimal(
                "risk.capital_pools.capital_admission_policy.fee_slippage.max_slippage_liability",
                &policy.fee_slippage.max_slippage_liability,
            )?,
        }),
    })
}

fn required_pool_decimal(label: &str, value: &str) -> Result<Decimal, BoltV3LiveNodeError> {
    parse_decimal_string(value).map_err(|message| {
        BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be a decimal string: {message}"
        ))
    })
}

fn optional_pool_decimal(
    label: &str,
    value: Option<&str>,
) -> Result<Option<Decimal>, BoltV3LiveNodeError> {
    value
        .map(|value| required_pool_decimal(label, value))
        .transpose()
}

pub(super) fn loss_governor_policy_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LossGovernorPolicy>, BoltV3LiveNodeError> {
    let Some(block) = loaded.root.risk.loss_governor.as_ref() else {
        return Ok(None);
    };
    if !block.enabled {
        return Ok(None);
    }
    Ok(Some(LossGovernorPolicy {
        max_snapshot_age_ns: block.max_snapshot_age_ns,
        max_per_trade_loss: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_per_trade_loss",
            block.max_per_trade_loss.as_deref(),
        )?),
        max_daily_loss: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_daily_loss",
            block.max_daily_loss.as_deref(),
        )?),
        max_rolling_loss: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_rolling_loss",
            block.max_rolling_loss.as_deref(),
        )?),
        max_drawdown: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_drawdown",
            block.max_drawdown.as_deref(),
        )?),
    }))
}

pub(super) fn loss_governor_halt_action_policy_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LossGovernorHaltActionPolicy>, BoltV3LiveNodeError> {
    let Some(block) = loaded.root.risk.loss_governor.as_ref() else {
        return Ok(None);
    };
    if !block.enabled {
        return Ok(None);
    }
    Ok(Some(LossGovernorHaltActionPolicy {
        on_loss_breach_trading_state: required_loss_governor_trading_state_action(
            "risk.loss_governor.on_loss_breach_trading_state",
            block.on_loss_breach_trading_state,
        )?,
        on_untrusted_snapshot_trading_state: required_loss_governor_trading_state_action(
            "risk.loss_governor.on_untrusted_snapshot_trading_state",
            block.on_untrusted_snapshot_trading_state,
        )?,
        recovery_mode: required_loss_governor_recovery_mode(
            "risk.loss_governor.recovery_mode",
            block.recovery_mode,
        )?,
        manual_recovery_evidence_max_path_bytes: required_loss_governor_usize(
            "risk.loss_governor.manual_recovery_evidence_max_path_bytes",
            block.manual_recovery_evidence_max_path_bytes,
        )?,
    }))
}

fn required_loss_governor_trading_state_action(
    label: &'static str,
    value: Option<LossGovernorTradingStateAction>,
) -> Result<LossGovernorTradingStateAction, BoltV3LiveNodeError> {
    value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))
}

fn required_loss_governor_recovery_mode(
    label: &'static str,
    value: Option<LossGovernorRecoveryMode>,
) -> Result<LossGovernorRecoveryMode, BoltV3LiveNodeError> {
    value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))
}

fn required_loss_governor_usize(
    label: &'static str,
    value: Option<usize>,
) -> Result<usize, BoltV3LiveNodeError> {
    let value =
        value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))?;
    if value == usize::MIN {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be positive"
        )));
    }
    Ok(value)
}

/// Reads the durable kill-switch state before the live node is built.
///
/// Enabled kill-switch boot must fail closed before resolving secrets,
/// constructing NT clients, or registering submit-capable strategy runtime when
/// the durable store holds an unresolved/corrupt/missing record. A disabled (or
/// unconfigured) kill switch carries no durable state requirement.
pub(super) fn recover_kill_switch_state_before_live_node_build(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<KillSwitchState>, BoltV3LiveNodeError> {
    let Some(config) = loaded.root.risk.kill_switch.as_ref() else {
        return Ok(None);
    };
    if !config.enabled {
        return Ok(None);
    }

    let store = KillSwitchStore::from_root_config_path(&loaded.root_path, config);
    match store
        .load_recovery_state()
        .map_err(BoltV3LiveNodeError::KillSwitchStore)?
    {
        KillSwitchRecoveryState::Recovered(state) => Ok(Some(state)),
        KillSwitchRecoveryState::FailClosed { reason, .. } => {
            Err(BoltV3LiveNodeError::KillSwitchRecovery { reason })
        }
    }
}

/// Syncs a recovered/seeded kill-switch state into NT's `RiskEngine` trading
/// state so the NT risk engine and the submit-admission latch agree on the halt
/// after a restart, instead of leaving NT trading `Active` behind a latched
/// admission.
pub(super) fn sync_nt_trading_state_for_kill_switch(node: &mut LiveNode, state: &KillSwitchState) {
    let Some(trading_state) = nt_trading_state_for_kill_switch_state(state) else {
        return;
    };
    node.kernel()
        .risk_engine()
        .borrow_mut()
        .set_trading_state(trading_state);
}

fn nt_trading_state_for_kill_switch_state(state: &KillSwitchState) -> Option<TradingState> {
    match state {
        KillSwitchState::Armed => None,
        KillSwitchState::Halting { .. }
        | KillSwitchState::Halted { .. }
        | KillSwitchState::Cancelling { .. }
        | KillSwitchState::Flattening { .. } => Some(TradingState::Reducing),
        KillSwitchState::Flat { .. } | KillSwitchState::FailedManualIntervention { .. } => {
            Some(TradingState::Halted)
        }
    }
}

/// Moves the NT risk engine to `Reducing` after a loss-halt action. Abstracted
/// as a trait so the hard kill-switch sink is unit-testable without a live NT
/// risk engine. The transition is venue-neutral: it applies regardless of the
/// global execution mode and does not submit, cancel, or close venue orders.
trait TradingStateController {
    fn enter_reducing(&self);
}

/// Closure-backed `TradingStateController`. The production caller captures the
/// NT risk-engine handle in the closure so the concrete NT risk-engine type
/// does not have to be named here; tests inject a recording controller instead.
struct ClosureTradingStateController<F: Fn()> {
    enter_reducing: F,
}

impl<F: Fn()> TradingStateController for ClosureTradingStateController<F> {
    fn enter_reducing(&self) {
        (self.enter_reducing)();
    }
}

trait KillSwitchFlattenExecutor {
    fn execute_flatten(&self, action: &KillSwitchLossAction) -> Result<()>;
}

/// Hard kill-switch loss-action sink that drives the NT runtime on a durable
/// daily-realized loss breach.
///
/// On a fresh `FlattenPositions` halt it moves the NT risk engine to
/// `Reducing`, then optionally routes the validated flatten commands through
/// the shared execution policy. Flatten failures are logged and evidenced by the
/// route where possible, but this sink never returns them to the durable halt
/// state machine: flatten is an effect of a halt, not the halt-state owner.
struct NtReducingLossActionSink {
    trading_state: Rc<dyn TradingStateController>,
    flatten_executor: Option<Rc<dyn KillSwitchFlattenExecutor>>,
    dispatched_halts: RefCell<BTreeSet<String>>,
}

impl NtReducingLossActionSink {
    fn new(trading_state: Rc<dyn TradingStateController>) -> Self {
        Self {
            trading_state,
            flatten_executor: None,
            dispatched_halts: RefCell::new(BTreeSet::new()),
        }
    }

    fn with_flatten_executor(
        trading_state: Rc<dyn TradingStateController>,
        flatten_executor: Rc<dyn KillSwitchFlattenExecutor>,
    ) -> Self {
        Self {
            trading_state,
            flatten_executor: Some(flatten_executor),
            dispatched_halts: RefCell::new(BTreeSet::new()),
        }
    }
}

impl KillSwitchLossActionSink for NtReducingLossActionSink {
    fn emit(&self, action: KillSwitchLossAction) -> Result<()> {
        if action.kind != KillSwitchLossActionKind::FlattenPositions {
            return Ok(());
        }
        if self.dispatched_halts.borrow().contains(&action.halt_id) {
            return Ok(());
        }
        self.trading_state.enter_reducing();
        if let Some(flatten_executor) = self.flatten_executor.as_ref()
            && let Err(error) = flatten_executor.execute_flatten(&action)
        {
            log::error!(
                "kill switch flatten action failed after reducing latch: halt_id={} action_id={}: {error:#}",
                action.halt_id,
                action.action_id
            );
        }
        self.dispatched_halts
            .borrow_mut()
            .insert(action.halt_id.clone());
        Ok(())
    }
}

fn live_node_kill_switch_flatten_executor(
    loaded: &LoadedBoltV3Config,
    node: &LiveNode,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
) -> Result<Option<Rc<dyn KillSwitchFlattenExecutor>>, BoltV3LiveNodeError> {
    let Some(kill_switch) = loaded
        .root
        .risk
        .kill_switch
        .as_ref()
        .filter(|kill_switch| kill_switch.enabled)
    else {
        return Ok(None);
    };
    let forced_policy = kill_switch_forced_reduction_policy_from_config(kill_switch)
        .map_err(BoltV3LiveNodeError::KillSwitchLossProtection)?;
    submit_admission.configure_kill_switch_forced_reduction_policy(forced_policy);

    if !kill_switch.flatten_open_positions_on_breach {
        return Ok(None);
    }

    let flatten = kill_switch
        .flatten
        .as_ref()
        .filter(|flatten| flatten.enabled)
        .ok_or_else(|| {
            BoltV3LiveNodeError::KillSwitchLossProtection(anyhow::anyhow!(
                "risk.kill_switch.flatten_open_positions_on_breach=true requires risk.kill_switch.flatten.enabled=true"
            ))
        })?;
    if flatten.route_kind != KillSwitchFlattenRouteKindConfig::LiveNodeCommandRouter {
        return Err(BoltV3LiveNodeError::KillSwitchLossProtection(
            anyhow::anyhow!(
                "risk.kill_switch.flatten_open_positions_on_breach=true requires risk.kill_switch.flatten.route_kind=live_node_command_router"
            ),
        ));
    }

    let flatten_policy = flatten_policy_from_config(flatten)
        .map_err(BoltV3LiveNodeError::KillSwitchLossProtection)?;
    let order_template = flatten_order_template_from_config(flatten);
    let execution_clients_by_venue = execution_clients_by_venue(loaded)
        .map_err(BoltV3LiveNodeError::KillSwitchLossProtection)?;
    let cache = node.kernel().cache();
    let risk_engine = node.kernel().risk_engine().clone();
    let clock = node.kernel().clock();
    let trader_id = node.kernel().trader_id();
    let order_execution_policy =
        BoltV3OrderExecutionPolicy::from_mode(loaded.root.runtime.order_execution_mode);
    let config_sha256 = loaded.config_bundle_checksum.clone();
    let policy_sha256 = kill_switch.forced_reduction_policy_sha256.clone();
    let executor = ClosureKillSwitchFlattenExecutor {
        execute_flatten: move |action: &KillSwitchLossAction| {
            let observed_at_unix_nanos = current_unix_nanos()?;
            let candidates = kill_switch_flatten_candidates_from_cache(
                &cache.borrow(),
                action,
                observed_at_unix_nanos,
            )?;
            let snapshot =
                BoltV3KillSwitchFlattenSnapshot::new(candidates).map_err(domain_error)?;
            let claim = BoltV3KillSwitchForcedReductionClaim::new(
                action.halt_id.clone(),
                action.action_id.clone(),
                policy_sha256.clone(),
            )
            .map_err(domain_error)?;
            let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(
                BoltV3KillSwitchFlattenPlanRequest {
                    kill_switch_state: KillSwitchState::Flattening {
                        halt_id: action.halt_id.clone(),
                    },
                    nt_trading_state: TradingState::Reducing,
                    action_id: action.action_id.clone(),
                    config_sha256: config_sha256.clone(),
                    policy_sha256: policy_sha256.clone(),
                    source_timestamp_unix_nanos: observed_at_unix_nanos,
                    policy: flatten_policy,
                    snapshot,
                    observed_at_unix_nanos,
                    route_proof: BoltV3KillSwitchFlattenRouteProof::new(
                        BoltV3KillSwitchFlattenRouteKind::LiveNodeCommandRouter,
                    ),
                    order_template: order_template.clone(),
                    forced_reduction_claim: claim,
                },
            )
            .map_err(domain_error)?;

            route_planned_kill_switch_flatten_commands(&plan, |command| {
                let instrument = {
                    let cache = cache.borrow();
                    cache.instrument(&command.instrument_id()).cloned()
                }
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "kill switch flatten instrument not found in NT cache: instrument_id={}",
                        command.instrument_id()
                    )
                })?;
                let venue = command.instrument_id().venue;
                let execution_client_id = execution_clients_by_venue
                    .get(&venue)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "kill switch flatten execution client not configured for venue={venue}"
                        )
                    })?
                    .as_str();
                let mut order_factory = OrderFactory::new(
                    trader_id,
                    command.strategy_id(),
                    None,
                    None,
                    clock.clone(),
                    false,
                    true,
                );
                let mut sink = BoltV3NtSubmitOnlySink::new(|order, context| {
                    if order.status() != nautilus_model::enums::OrderStatus::Initialized {
                        anyhow::bail!(
                            "kill switch flatten order denied before NT risk engine: invalid status for {}, expected INITIALIZED",
                            order.client_order_id()
                        );
                    }
                    {
                        cache.borrow_mut().add_order(
                            order.clone(),
                            context.position_id,
                            context.client_id,
                            true,
                        )?;
                    }
                    publish_order_initialized(&order);
                    let params = context.params.filter(|params| !params.is_empty());
                    let command = SubmitOrder::new(
                        trader_id,
                        context.client_id,
                        order.strategy_id(),
                        order.instrument_id(),
                        order.client_order_id(),
                        order.init_event().clone(),
                        order.exec_algorithm_id(),
                        context.position_id,
                        params,
                        UUID4::new(),
                        clock.borrow().timestamp_ns(),
                        None,
                    );
                    risk_engine
                        .borrow_mut()
                        .execute(TradingCommand::SubmitOrder(command));
                    Ok(())
                });
                let fallback_price = instrument
                    .max_price()
                    .map(|price| price.to_string())
                    .unwrap_or_else(|| Decimal::ZERO.to_string());
                route_kill_switch_flatten_command_with_sink(
                    order_execution_policy,
                    &mut sink,
                    &mut order_factory,
                    decision_evidence.as_ref(),
                    submit_admission.as_ref(),
                    BoltV3KillSwitchFlattenRoutingContext {
                        execution_client_id,
                        fallback_price: fallback_price.as_str(),
                        instrument: Some(&instrument),
                        max_fee_bps: Decimal::ZERO,
                        submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
                    },
                    command,
                )?;
                Ok(())
            })?;
            Ok(())
        },
    };

    Ok(Some(Rc::new(executor)))
}

fn route_planned_kill_switch_flatten_commands<F>(
    plan: &BoltV3KillSwitchFlattenPlan,
    mut route_command: F,
) -> Result<()>
where
    F: FnMut(&BoltV3KillSwitchFlattenCommand) -> Result<()>,
{
    let mut failures = Vec::new();
    for command in plan.commands() {
        if let Err(error) = route_command(command) {
            log::error!(
                "kill switch flatten command failed: halt_id={} action_id={} position_id={} instrument_id={}: {error:#}",
                command.halt_id(),
                command.action_id(),
                command.position_id(),
                command.instrument_id()
            );
            failures.push(format!(
                "position_id={} instrument_id={}: {error:#}",
                command.position_id(),
                command.instrument_id()
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "kill switch flatten command failures: halt_id={} failures={}",
            plan.halt_id(),
            failures.join(" | ")
        )
    }
}

fn publish_order_initialized(order: &OrderAny) {
    let event = OrderEventAny::Initialized(order.init_event().clone());
    let topic = format!("events.order.{}", order.strategy_id());
    msgbus::publish_order_event(topic.into(), &event);
}

fn kill_switch_flatten_candidates_from_cache(
    cache: &nautilus_common::cache::Cache,
    action: &KillSwitchLossAction,
    observed_at_unix_nanos: u64,
) -> Result<Vec<BoltV3KillSwitchFlattenCandidate>> {
    let mut candidates = Vec::new();
    for account_id in &action.account_ids {
        let account_id = AccountId::from(account_id.as_str());
        for instrument_id in &action.instrument_ids {
            let instrument_id = InstrumentId::from_str(instrument_id)?;
            for position in
                cache.positions_open(None, Some(&instrument_id), None, Some(&account_id), None)
            {
                let source_timestamp_unix_nanos = position.ts_last.as_u64();
                let source_timestamp_unix_nanos = if source_timestamp_unix_nanos == 0 {
                    observed_at_unix_nanos
                } else {
                    source_timestamp_unix_nanos
                };
                candidates.push(
                    BoltV3KillSwitchFlattenCandidate::from_nt_position_state(
                        BoltV3KillSwitchFlattenPositionState {
                            evidence_kind:
                                BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
                            account_id: position.account_id,
                            instrument_id: position.instrument_id,
                            strategy_id: position.strategy_id,
                            position_id: position.id,
                            position_side: position.side,
                            quantity: position.quantity,
                            source_timestamp_unix_nanos,
                        },
                    )
                    .map_err(domain_error)?,
                );
            }
        }
    }
    Ok(candidates)
}

fn domain_error(error: impl std::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{error:?}")
}

fn kill_switch_forced_reduction_policy_from_config(
    kill_switch: &crate::bolt_v3_config::KillSwitchConfigBlock,
) -> Result<BoltV3KillSwitchForcedReductionPolicy> {
    let max_notional = parse_decimal_string(&kill_switch.forced_reduction_max_notional_per_order)
        .map_err(|reason| anyhow::anyhow!("{reason}"))?;
    BoltV3KillSwitchForcedReductionPolicy::new(
        kill_switch.forced_reduction_policy_sha256.clone(),
        kill_switch.forced_reduction_max_live_order_count,
        max_notional,
    )
    .map_err(|error| anyhow::anyhow!("{error:?}"))
}

fn flatten_policy_from_config(
    _flatten: &KillSwitchFlattenConfigBlock,
) -> Result<BoltV3KillSwitchFlattenPolicy> {
    Ok(BoltV3KillSwitchFlattenPolicy::new())
}

fn flatten_order_template_from_config(flatten: &KillSwitchFlattenConfigBlock) -> NtOrderTemplate {
    NtOrderTemplate {
        order_type: flatten.order_type,
        time_in_force: flatten.time_in_force,
        expire_time: None,
        trigger_price: None,
        activation_price: None,
        trigger_type: None,
        trigger_instrument_id: None,
        trailing_offset: None,
        trailing_offset_type: None,
        is_post_only: flatten.is_post_only,
        is_reduce_only: flatten.is_reduce_only,
        is_quote_quantity: flatten.is_quote_quantity,
    }
}

fn execution_clients_by_venue(loaded: &LoadedBoltV3Config) -> Result<BTreeMap<Venue, String>> {
    let mut clients_by_venue = BTreeMap::new();
    for (client_key, client) in loaded
        .root
        .clients
        .iter()
        .filter(|(_, client)| client.execution.is_some())
    {
        let venue = Venue::from(client.venue.as_str());
        if let Some(existing) = clients_by_venue.insert(venue, client_key.clone()) {
            anyhow::bail!(
                "kill switch flatten requires one execution client per venue; venue={venue} clients={existing},{client_key}"
            );
        }
    }
    Ok(clients_by_venue)
}

/// Configures the durable kill-switch loss-protection accumulator from the
/// validated `risk.kill_switch` block.
///
/// Returns `Ok(None)` when the kill switch is absent or disabled. Otherwise it
/// builds the daily-realized accumulator, wires its live action to the NT
/// `Reducing` trading-state transition, then seeds it from the durable store. A
/// failed seed can fail closed to a halted state; the caller syncs NT trading
/// state from the resulting state.
pub(super) fn configure_bolt_v3_kill_switch_loss_protection(
    loaded: &LoadedBoltV3Config,
    node: &LiveNode,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
) -> Result<Option<Rc<RefCell<KillSwitchLossProtection>>>, BoltV3LiveNodeError> {
    let Some(kill_switch) = loaded
        .root
        .risk
        .kill_switch
        .as_ref()
        .filter(|kill_switch| kill_switch.enabled)
    else {
        return Ok(None);
    };
    let max_utc_daily_realized_loss =
        parse_decimal_string(&kill_switch.max_utc_daily_realized_loss).map_err(|reason| {
            BoltV3LiveNodeError::KillSwitchLossProtection(anyhow::anyhow!(
                "risk.kill_switch.max_utc_daily_realized_loss parse failed: {reason}"
            ))
        })?;
    let config = KillSwitchLossProtectionConfig {
        max_utc_daily_realized_loss,
        action_retry_interval_ms: kill_switch.action_retry_interval_ms,
        action_retry_timeout_ms: kill_switch.action_retry_timeout_ms,
        account_ids: kill_switch.account_ids.clone(),
        instrument_ids: kill_switch.instrument_ids.clone(),
    };
    let store = KillSwitchStore::from_root_config_path(&loaded.root_path, kill_switch);
    let risk_engine = node.kernel().risk_engine().clone();
    let trading_state: Rc<dyn TradingStateController> = Rc::new(ClosureTradingStateController {
        enter_reducing: move || {
            risk_engine
                .borrow_mut()
                .set_trading_state(TradingState::Reducing);
        },
    });
    let flatten_executor = live_node_kill_switch_flatten_executor(
        loaded,
        node,
        decision_evidence,
        submit_admission.clone(),
    )?;
    let action_sink: Rc<dyn KillSwitchLossActionSink> = match flatten_executor {
        Some(flatten_executor) => Rc::new(NtReducingLossActionSink::with_flatten_executor(
            trading_state,
            flatten_executor,
        )),
        None => Rc::new(NtReducingLossActionSink::new(trading_state)),
    };
    let mut protection =
        KillSwitchLossProtection::new(config, submit_admission, store, action_sink)
            .map_err(BoltV3LiveNodeError::KillSwitchLossProtection)?;
    let recovery_action_clock_unix_nanos =
        current_unix_nanos().map_err(BoltV3LiveNodeError::KillSwitchLossProtection)?;
    protection
        .seed_from_store(recovery_action_clock_unix_nanos)
        .map_err(BoltV3LiveNodeError::KillSwitchLossProtection)?;
    Ok(Some(Rc::new(RefCell::new(protection))))
}

/// Runtime guards for the durable kill-switch loss protection. Owns the
/// position-event subscription and the action-retry task; dropping it
/// unsubscribes and aborts the retry loop.
pub(super) struct BoltV3LossProtectionRuntimeGuards {
    position_events: Option<TypedHandler<PositionEvent>>,
    retry_handle: Option<tokio::task::JoinHandle<()>>,
}

impl BoltV3LossProtectionRuntimeGuards {
    fn none() -> Self {
        Self {
            position_events: None,
            retry_handle: None,
        }
    }

    pub(super) async fn stop_and_join(mut self) {
        self.unsubscribe_position_events();
        let retry_handle = self.retry_handle.take();
        drop(self);
        if let Some(retry_handle) = retry_handle {
            retry_handle.abort();
            match retry_handle.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    log::error!(
                        "bolt-v3 kill-switch loss protection retry task join failed: {error:?}"
                    );
                }
            }
        }
    }

    fn unsubscribe_position_events(&mut self) {
        if let Some(position_events) = self.position_events.take() {
            unsubscribe_position_events(position_events_pattern(), &position_events);
        }
    }

    fn abort_retry_task(&mut self) {
        if let Some(retry_handle) = self.retry_handle.take() {
            retry_handle.abort();
        }
    }
}

impl Drop for BoltV3LossProtectionRuntimeGuards {
    fn drop(&mut self) {
        self.unsubscribe_position_events();
        self.abort_retry_task();
    }
}

/// Wires the durable kill-switch loss protection into NT's runtime: subscribes
/// the accumulator to position events (so realized PnL drives the daily breach)
/// and spawns the pending-halt-action retry loop. A no-op when no kill switch
/// is configured.
pub(super) fn wire_bolt_v3_loss_protection_runtime(
    runtime: &BoltV3LiveNodeRuntime,
) -> BoltV3LossProtectionRuntimeGuards {
    let Some(loss_protection) = runtime.loss_protection.as_ref() else {
        return BoltV3LossProtectionRuntimeGuards::none();
    };
    let retry_interval_ms = loss_protection.borrow().action_retry_interval_ms();
    let retry_loss_protection = Rc::clone(loss_protection);
    let retry_handle = tokio::task::spawn_local(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(retry_interval_ms));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let now_unix_nanos = match current_unix_nanos() {
                Ok(now_unix_nanos) => now_unix_nanos,
                Err(error) => {
                    log::error!("bolt-v3 kill-switch loss protection retry clock failed: {error}");
                    continue;
                }
            };
            if let Err(error) = retry_loss_protection
                .borrow_mut()
                .poll_pending_halt_actions(now_unix_nanos)
            {
                log::error!("bolt-v3 kill-switch loss protection action retry failed: {error}");
            }
        }
    });
    let loss_protection = Rc::clone(loss_protection);
    let position_events = TypedHandler::from(move |event: &PositionEvent| {
        if let Err(error) = loss_protection.borrow_mut().record_position_event(event) {
            log::error!(
                "bolt-v3 kill-switch loss protection position-event handling failed: {error}"
            );
        }
    });
    subscribe_position_events(position_events_pattern(), position_events.clone(), None);
    BoltV3LossProtectionRuntimeGuards {
        position_events: Some(position_events),
        retry_handle: Some(retry_handle),
    }
}

pub(super) fn loss_governor_halt_action_handler_from_node(
    node: &LiveNode,
    loss_policy: LossGovernorPolicy,
    action_policy: LossGovernorHaltActionPolicy,
) -> LossGovernorHaltActionHandler {
    // The handler only needs to read and flip NT's `RiskEngine` trading state; the
    // node is solely the source of that engine handle. Delegate to the node-free
    // core via trading-state accessors so the halt behaviour can be exercised
    // without building a `NautilusKernel`, whose logging init claims the
    // process-global `log` slot and is mutually exclusive with a test's
    // capturing logger.
    let read_engine = node.kernel().risk_engine().clone();
    let write_engine = read_engine.clone();
    loss_governor_halt_action_handler(
        Rc::new(move || read_engine.borrow().trading_state()),
        Rc::new(move |state| write_engine.borrow_mut().set_trading_state(state)),
        loss_policy,
        action_policy,
    )
}

/// Node-free core of the loss-governor halt action handler. Reads and flips the
/// trading state via the supplied accessors rather than a `LiveNode`/`RiskEngine`
/// handle, so the halt logic can be exercised against plain state cells without
/// building a `NautilusKernel` (whose logging init owns the global `log` slot).
fn loss_governor_halt_action_handler(
    read_trading_state: Rc<dyn Fn() -> TradingState>,
    set_trading_state: Rc<dyn Fn(TradingState)>,
    loss_policy: LossGovernorPolicy,
    action_policy: LossGovernorHaltActionPolicy,
) -> LossGovernorHaltActionHandler {
    Rc::new(move |snapshot, now_ns, source_observations| {
        let decision = evaluate_loss_admission_with_observations(
            &loss_policy,
            snapshot,
            now_ns,
            source_observations,
        );
        let current_state = read_trading_state();
        if current_state == TradingState::Active && decision.accepted {
            return;
        }

        if let Some(target_state) =
            next_loss_governor_trading_state(&action_policy, current_state, &decision)
        {
            // Apply the trading-state transition unconditionally — the
            // loss-governor halt itself MUST always fire.
            set_trading_state(target_state);
        }
    })
}

fn required_loss_governor_decimal(
    label: &'static str,
    value: Option<&str>,
) -> Result<Decimal, BoltV3LiveNodeError> {
    let value =
        value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))?;
    let decimal = parse_decimal_string(value).map_err(|reason| {
        BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be a valid decimal string ({reason}): `{value}`"
        ))
    })?;
    if decimal <= Decimal::ZERO {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be positive: `{value}`"
        )));
    }
    Ok(decimal)
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::BTreeMap,
        panic::{AssertUnwindSafe, catch_unwind},
        rc::Rc,
        sync::Mutex,
    };

    use super::*;

    use crate::{
        bolt_v3_capital_admission::{
            CapitalAdmissionPolicy, FeeSlippagePolicy, PredictionMarketAdmissionSnapshot,
            ProductAdmissionSnapshot, ProductKind,
        },
        bolt_v3_capital_admission_runtime_feed::{
            CapitalAdmissionRuntimeFeed, CapitalAdmissionRuntimeFeedConfig,
            POLYMARKET_VENUE_TRUTH_REST_SOURCE,
        },
        bolt_v3_capital_admission_state::{
            OrderLifecycleCapitalAdmissionSnapshot, PortfolioCapitalAdmissionSnapshot,
            VenueSpendabilitySnapshot,
        },
        bolt_v3_capital_reservation::CapitalPoolSnapshot,
        bolt_v3_decision_evidence::{
            BoltV3AdmissionDecisionEvidence, BoltV3BasketAdmissionDecisionEvidence,
            BoltV3CapitalAdmissionRebuildAuditEvidence, BoltV3DecisionEvidenceWriter,
            BoltV3EntrySkipEvidence, BoltV3ExitDecisionEvidence, BoltV3ExitEvaluationEvidence,
            BoltV3LossGovernorHaltEvidence, BoltV3OrderIntentClampOutcome,
            BoltV3OrderIntentEvidence, BoltV3OrderRejectEvidence, BoltV3RequoteThrottleEvidence,
            BoltV3StrategyInputEvidenceSnapshot, BoltV3SubmitReservationFillEvidence,
            BoltV3SubmitReservationMetadataEvidence,
        },
        bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState, KillSwitchStateKind},
        bolt_v3_kill_switch_store::{KillSwitchRecoveryState, KillSwitchStore},
        bolt_v3_order_execution::{
            BoltV3KillSwitchFlattenRoutingContext, BoltV3NtSubmitOnlySink,
            BoltV3OrderExecutionPolicy, route_kill_switch_flatten_command_with_sink,
        },
        bolt_v3_order_intent::NtOrderTemplate,
        bolt_v3_submit_admission::{
            BoltV3KillSwitchForcedReductionClaim, BoltV3KillSwitchForcedReductionPolicy,
            BoltV3SubmitAdmissionState, BoltV3SubmitCapitalAdmissionConfig, BoltV3SubmitIntentKind,
            BoltV3SubmitLifecyclePolicy,
        },
        bolt_v3_venue_truth::{
            VenueTruthCaptureEndpointError, VenueTruthDivergence, VenueTruthDivergenceAlarmClass,
            VenueTruthDivergenceEvidence, VenueTruthDivergenceKind, VenueTruthSnapshot,
        },
    };
    use anyhow::Result;
    use nautilus_common::{
        clock::{Clock, TestClock},
        factories::OrderFactory,
    };
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        enums::{AssetClass, OrderType, PositionSide, TimeInForce, TradingState},
        identifiers::{AccountId, InstrumentId, PositionId, StrategyId, Symbol, TraderId},
        instruments::{BinaryOption, InstrumentAny},
        orders::Order,
        types::{Currency, Money, Price, Quantity},
    };
    use ustr::Ustr;

    #[test]
    fn venue_truth_capture_failure_evidence_uses_production_endpoint_error_parts() {
        let error = anyhow::anyhow!(VenueTruthCaptureEndpointError::new(
            "clob_balance_allowance",
            "transport_or_decode",
            anyhow::anyhow!("transport failed"),
        ))
        .context("poll venue truth");

        let evidence = venue_truth_capture_failure_evidence(1_100, 3, &error);

        assert_eq!(
            evidence.source,
            crate::bolt_v3_capital_admission_runtime_feed::POLYMARKET_VENUE_TRUTH_REST_SOURCE
        );
        assert_eq!(evidence.observed_at_ns, 1_100);
        assert_eq!(evidence.endpoint, "clob_balance_allowance");
        assert_eq!(evidence.error_class, "transport_or_decode");
        assert_eq!(evidence.captures_missed, 3);
    }

    #[test]
    fn venue_truth_capture_failure_handler_suspends_without_durable_halt() {
        let admission = BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(NoStrategyDecisionEvidenceWriter),
            BoltV3SubmitCapitalAdmissionConfig {
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                product_kind: ProductKind::PredictionMarketBinary,
                collateral_currency: "USD".to_string(),
                capital_pool: CapitalPoolSnapshot {
                    source: "test".to_string(),
                    observed_at_ns: 900,
                    pool_id: "pool-1".to_string(),
                    max_pool_liability: Decimal::new(10, 0),
                    committed_liability: Decimal::ZERO,
                    max_snapshot_age_ns: 500,
                },
                policy: CapitalAdmissionPolicy {
                    min_remaining_pool_balance: None,
                    fee_slippage_policy: None,
                },
                dedupe_retention_ns: 500,
            },
        );
        let error = anyhow::anyhow!(VenueTruthCaptureEndpointError::new(
            "clob_open_orders",
            "transport_or_decode",
            anyhow::anyhow!("transport failed"),
        ));

        handle_venue_truth_capture_failure(&admission, 1_200, 2, &error);

        assert_eq!(
            admission.kill_switch_state_kind(),
            KillSwitchStateKind::Armed
        );
        assert_eq!(admission.capital_admission_reconciled(), Some(false));
    }

    #[test]
    fn venue_truth_failure_recovery_repeat_failure_emits_three_health_transitions() {
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(NoStrategyDecisionEvidenceWriter),
            test_capital_admission_config(),
        ));
        let feed = Arc::new(Mutex::new(CapitalAdmissionRuntimeFeed::new(
            test_capital_admission_runtime_feed_config(),
            admission.clone(),
        )));
        let logger = BoltV3OperatorHealthTransitionLogger::new();
        let emissions = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let health_emitter: BoltV3OperatorHealthTransitionEmitter = {
            let admission = admission.clone();
            let logger = logger.clone();
            let emissions = emissions.clone();
            Arc::new(move |reason| {
                let surface = live_operator_health_surface(None, &admission, true, 0, None);
                if logger.emit_surface(reason, surface)
                    == BoltV3OperatorHealthTransitionEmission::Emitted
                {
                    emissions
                        .lock()
                        .expect("test emissions lock should not be poisoned")
                        .push(reason);
                }
            })
        };
        let mut baseline = test_venue_truth_snapshot();
        baseline.captured_at = UnixNanos::from(1_100);
        reconcile_venue_truth_snapshot(&feed, baseline)
            .expect("initial venue truth snapshot should seed nominal health");
        assert_eq!(admission.capital_admission_reconciled(), Some(true));

        let error = anyhow::anyhow!(VenueTruthCaptureEndpointError::new(
            "clob_open_orders",
            "transport_or_decode",
            anyhow::anyhow!("transport failed"),
        ));

        handle_venue_truth_capture_failure(&admission, 1_200, 1, &error);
        health_emitter(OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_FAILURE);

        let mut recovery = test_venue_truth_snapshot();
        recovery.captured_at = UnixNanos::from(1_300);
        reconcile_venue_truth_snapshot_with_health_emission(&feed, recovery, Some(&health_emitter))
            .expect("accepted venue truth snapshot should reconcile");

        handle_venue_truth_capture_failure(&admission, 1_400, 1, &error);
        health_emitter(OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_FAILURE);

        let emissions = emissions
            .lock()
            .expect("test emissions lock should not be poisoned")
            .clone();
        assert_eq!(
            emissions,
            vec![
                OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_FAILURE,
                OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_RECOVERY,
                OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_FAILURE,
            ]
        );
    }

    #[test]
    fn venue_truth_divergence_halt_persists_halted_from_recovered_store() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let store = KillSwitchStore::new(temp.path().join("kill-switch.json"), 65_536);
        store
            .write_state(&KillSwitchState::Armed)
            .expect("recovered armed state should persist");
        let admission = BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(NoStrategyDecisionEvidenceWriter),
            test_capital_admission_config(),
        );

        let state = durably_halt_for_venue_truth_divergence(
            &admission,
            &store,
            test_venue_truth_divergence(),
        );

        assert_eq!(state.kind(), KillSwitchStateKind::Halted);
        assert_eq!(
            admission.kill_switch_state_kind(),
            KillSwitchStateKind::Halted
        );
        let recovered = store
            .load_recovery_state()
            .expect("persisted venue truth halt should load");
        let KillSwitchRecoveryState::Recovered(KillSwitchState::Halted { trigger, .. }) = recovered
        else {
            panic!("venue truth divergence should persist a recovered halted state");
        };
        assert_eq!(
            trigger.kind,
            crate::bolt_v3_kill_switch::KillSwitchHaltTriggerKind::VenueTruthDivergence
        );
        assert!(trigger.reason.contains("alarm_class=TrueDivergence"));
    }

    #[test]
    fn venue_truth_divergence_halt_records_decision_evidence_fields() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let store = KillSwitchStore::new(temp.path().join("kill-switch.json"), 65_536);
        store
            .write_state(&KillSwitchState::Armed)
            .expect("recovered armed state should persist");
        let writer = Arc::new(TestVenueTruthDivergenceEvidenceWriter::recording());
        let admission = BoltV3SubmitAdmissionState::new_with_capital_admission(
            writer.clone(),
            test_capital_admission_config(),
        );

        let state = durably_halt_for_venue_truth_divergence(
            &admission,
            &store,
            test_venue_truth_divergence(),
        );

        assert_eq!(state.kind(), KillSwitchStateKind::Halted);
        let records = writer.records();
        assert_eq!(records.len(), 1);
        let evidence = &records[0];
        assert_eq!(
            evidence.source,
            crate::bolt_v3_capital_admission_runtime_feed::POLYMARKET_VENUE_TRUTH_REST_SOURCE
        );
        assert_eq!(evidence.account_id, "ACCOUNT-001");
        assert_eq!(evidence.field, "collateral_balance");
        assert_eq!(evidence.venue_value, "75");
        assert_eq!(evidence.prior_accepted_value, "100");
        assert_eq!(
            evidence.missing_explanation,
            "no filled event explains collateral delta"
        );
        assert_eq!(
            evidence.alarm_class,
            VenueTruthDivergenceAlarmClass::TrueDivergence
        );
    }

    #[test]
    fn venue_truth_divergence_halt_does_not_downgrade_existing_non_armed_store_state() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let store = KillSwitchStore::new(temp.path().join("kill-switch.json"), 65_536);
        let existing = KillSwitchState::Halted {
            halt_id: "existing-halt".to_string(),
            trigger: KillSwitchHaltTrigger::loss_governor_breach(
                "loss-governor",
                1_000,
                "daily loss cap breached",
            ),
        };
        store
            .write_state(&existing)
            .expect("existing halted state should persist");
        let admission = BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(NoStrategyDecisionEvidenceWriter),
            test_capital_admission_config(),
        );

        let state = durably_halt_for_venue_truth_divergence(
            &admission,
            &store,
            test_venue_truth_divergence(),
        );

        assert_eq!(state, existing);
        assert_eq!(
            admission.kill_switch_state_kind(),
            KillSwitchStateKind::Halted
        );
        assert_eq!(
            store
                .load_recovery_state()
                .expect("existing halted state should remain readable"),
            KillSwitchRecoveryState::Recovered(existing)
        );
    }

    #[test]
    fn venue_truth_divergence_evidence_write_failure_latches_fail_closed() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let store = KillSwitchStore::new(temp.path().join("kill-switch.json"), 65_536);
        store
            .write_state(&KillSwitchState::Armed)
            .expect("recovered armed state should persist");
        let admission = BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(TestVenueTruthDivergenceEvidenceWriter::failing()),
            test_capital_admission_config(),
        );

        let state = durably_halt_for_venue_truth_divergence(
            &admission,
            &store,
            test_venue_truth_divergence(),
        );

        assert_eq!(state.kind(), KillSwitchStateKind::FailedManualIntervention);
        assert_eq!(
            admission.kill_switch_state_kind(),
            KillSwitchStateKind::FailedManualIntervention
        );
        let recovered = store
            .load_recovery_state()
            .expect("fail-closed venue truth halt should load");
        let KillSwitchRecoveryState::FailClosed {
            state: Some(KillSwitchState::FailedManualIntervention { reason, .. }),
            ..
        } = recovered
        else {
            panic!("evidence write failure should persist a fail-closed state");
        };
        assert!(
            reason.contains("decision evidence unavailable"),
            "persisted fail-closed state should carry the evidence persistence failure: {reason}"
        );
    }

    #[test]
    fn venue_truth_runtime_failure_latches_without_writing_durable_halt() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let store = KillSwitchStore::new(temp.path().join("kill-switch.json"), 65_536);
        store
            .write_state(&KillSwitchState::Armed)
            .expect("recovered armed state should persist");
        let admission = BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(NoStrategyDecisionEvidenceWriter),
            test_capital_admission_config(),
        );

        let state = latch_non_durable_venue_truth_runtime_failure(
            &admission,
            1_300,
            "clock failed before venue truth poll".to_string(),
        );

        assert_eq!(state.kind(), KillSwitchStateKind::FailedManualIntervention);
        assert_eq!(
            admission.kill_switch_state_kind(),
            KillSwitchStateKind::FailedManualIntervention
        );
        assert_eq!(
            store
                .load_recovery_state()
                .expect("runtime-failure should leave durable baseline readable"),
            KillSwitchRecoveryState::Recovered(KillSwitchState::Armed)
        );
    }

    #[test]
    fn venue_truth_divergence_kill_switch_write_failure_latches_fail_closed() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join("kill-switch.json");
        let bootstrap_store = KillSwitchStore::new(path.clone(), 65_536);
        bootstrap_store
            .write_state(&KillSwitchState::Armed)
            .expect("recovered armed state should persist");
        let armed_state_bytes = std::fs::metadata(&path)
            .expect("armed state metadata should read")
            .len();
        let constrained_store = KillSwitchStore::new(path, armed_state_bytes);
        let admission = BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(NoStrategyDecisionEvidenceWriter),
            test_capital_admission_config(),
        );

        let state = durably_halt_for_venue_truth_divergence(
            &admission,
            &constrained_store,
            test_venue_truth_divergence(),
        );

        assert_eq!(state.kind(), KillSwitchStateKind::FailedManualIntervention);
        let KillSwitchState::FailedManualIntervention { reason, .. } = &state else {
            panic!("write failure should return failed manual intervention");
        };
        assert!(
            reason.contains("kill switch state write failed"),
            "returned state should carry the original halted-state persistence failure: {reason}"
        );
        assert!(
            reason.contains("fail-closed kill switch state write failed"),
            "returned state should also carry the failed fail-closed persistence attempt: {reason}"
        );
        assert_eq!(
            admission.kill_switch_state_kind(),
            KillSwitchStateKind::FailedManualIntervention
        );
        let recovered = bootstrap_store
            .load_recovery_state()
            .expect("preexisting armed state should remain readable");
        assert_eq!(
            recovered,
            KillSwitchRecoveryState::Recovered(KillSwitchState::Armed),
            "when nothing can persist, the store must not pretend a durable halt exists"
        );
    }

    #[test]
    fn nt_reducing_loss_action_sink_routes_flatten_executor_once_per_halt() {
        let trading_state = Rc::new(RecordingTradingStateController::default());
        let flatten_executor = Rc::new(RecordingFlattenExecutor::recording());
        let sink = NtReducingLossActionSink::with_flatten_executor(
            trading_state.clone(),
            flatten_executor.clone(),
        );
        let action = test_flatten_action("halt-001");

        sink.emit(action.clone())
            .expect("flatten action should not degrade halt dispatch");
        sink.emit(action)
            .expect("duplicate halt action should stay idempotent");

        assert_eq!(trading_state.enter_reducing_calls(), 1);
        let actions = flatten_executor.actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].halt_id, "halt-001");
        assert_eq!(actions[0].kind, KillSwitchLossActionKind::FlattenPositions);
    }

    #[test]
    fn nt_reducing_loss_action_sink_does_not_return_flatten_executor_failure() {
        let trading_state = Rc::new(RecordingTradingStateController::default());
        let flatten_executor = Rc::new(RecordingFlattenExecutor::failing());
        let sink = NtReducingLossActionSink::with_flatten_executor(
            trading_state.clone(),
            flatten_executor.clone(),
        );

        sink.emit(test_flatten_action("halt-002"))
            .expect("flatten effect failure must not degrade durable halt dispatch");

        assert_eq!(trading_state.enter_reducing_calls(), 1);
        assert_eq!(flatten_executor.actions().len(), 1);
    }

    #[test]
    fn triggered_halt_without_flatten_executor_has_zero_submits_but_wired_sink_submits_clamped() {
        let pre_wiring_writer = Arc::new(RecordingFlattenDecisionEvidenceWriter::default());
        let pre_wiring_executor = Rc::new(RoutingFlattenExecutor::new(
            pre_wiring_writer.clone(),
            Decimal::new(3, 0),
        ));
        let pre_wiring_trading_state = Rc::new(RecordingTradingStateController::default());
        let pre_wiring_sink = NtReducingLossActionSink::new(pre_wiring_trading_state.clone());
        pre_wiring_sink
            .emit(test_flatten_action("halt-pre-wiring"))
            .expect("pre-wiring sink should only latch NT reducing");

        assert_eq!(pre_wiring_trading_state.enter_reducing_calls(), 1);
        assert_eq!(pre_wiring_executor.submitted_quantities(), Vec::new());
        assert_eq!(pre_wiring_writer.records(), Vec::new());
        assert_eq!(pre_wiring_writer.admission_decisions(), Vec::new());

        let writer = Arc::new(RecordingFlattenDecisionEvidenceWriter::default());
        let wired_executor = Rc::new(RoutingFlattenExecutor::new(
            writer.clone(),
            Decimal::new(3, 0),
        ));
        let wired_trading_state = Rc::new(RecordingTradingStateController::default());
        let wired_sink = NtReducingLossActionSink::with_flatten_executor(
            wired_trading_state.clone(),
            wired_executor.clone(),
        );

        wired_sink
            .emit(test_flatten_action("halt-wired"))
            .expect("wired sink should not degrade halt state when flatten routes");

        assert_eq!(wired_trading_state.enter_reducing_calls(), 1);
        assert_eq!(
            wired_executor.submitted_quantities(),
            vec![Quantity::new(3.0, 2)]
        );
        let records = writer.records();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].clamp_outcome,
            Some(BoltV3OrderIntentClampOutcome::Clamped {
                original_quantity: Quantity::new(5.0, 2).as_decimal().to_string(),
            })
        );
        let admission_decisions = writer.admission_decisions();
        assert_eq!(admission_decisions.len(), 1);
        assert_eq!(
            admission_decisions[0].intent_kind,
            BoltV3SubmitIntentKind::KillSwitchForcedReduction
        );
    }

    #[test]
    fn planned_flatten_command_routing_continues_after_command_failure() {
        let plan = two_command_flatten_plan("halt-loop");
        let mut routed_positions = Vec::new();

        let error = route_planned_kill_switch_flatten_commands(&plan, |command| {
            routed_positions.push(command.position_id().to_string());
            if command.position_id() == PositionId::from("POSITION-001") {
                anyhow::bail!("synthetic first command failure");
            }
            Ok(())
        })
        .expect_err("one command failure should return a loud aggregate error");

        assert_eq!(
            routed_positions,
            vec!["POSITION-001".to_string(), "POSITION-002".to_string()]
        );
        assert!(
            error
                .to_string()
                .contains("kill switch flatten command failures"),
            "unexpected aggregate error: {error:#}"
        );
        assert!(
            error.to_string().contains("POSITION-001"),
            "aggregate error should name the failed position: {error:#}"
        );
    }

    #[test]
    #[should_panic(expected = "venue truth reconcile feed lock poisoned")]
    fn venue_truth_reconcile_feed_lock_poison_panics() {
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(NoStrategyDecisionEvidenceWriter),
            BoltV3SubmitCapitalAdmissionConfig {
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                product_kind: ProductKind::PredictionMarketBinary,
                collateral_currency: "USD".to_string(),
                capital_pool: CapitalPoolSnapshot {
                    source: "test".to_string(),
                    observed_at_ns: 900,
                    pool_id: "pool-1".to_string(),
                    max_pool_liability: Decimal::new(10, 0),
                    committed_liability: Decimal::ZERO,
                    max_snapshot_age_ns: 500,
                },
                policy: CapitalAdmissionPolicy {
                    min_remaining_pool_balance: None,
                    fee_slippage_policy: None,
                },
                dedupe_retention_ns: 500,
            },
        ));
        let feed = Arc::new(Mutex::new(CapitalAdmissionRuntimeFeed::new(
            test_capital_admission_runtime_feed_config(),
            admission,
        )));
        let poisoned = catch_unwind(AssertUnwindSafe(|| {
            let _guard = feed.lock().unwrap();
            panic!("poison venue truth reconcile feed lock");
        }));
        assert!(poisoned.is_err());
        assert!(feed.lock().is_err());

        let _ = reconcile_venue_truth_snapshot(&feed, test_venue_truth_snapshot());
    }

    fn test_capital_admission_config() -> BoltV3SubmitCapitalAdmissionConfig {
        BoltV3SubmitCapitalAdmissionConfig {
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "USD".to_string(),
            capital_pool: CapitalPoolSnapshot {
                source: "test".to_string(),
                observed_at_ns: 900,
                pool_id: "pool-1".to_string(),
                max_pool_liability: Decimal::new(10, 0),
                committed_liability: Decimal::ZERO,
                max_snapshot_age_ns: 500,
            },
            policy: CapitalAdmissionPolicy {
                min_remaining_pool_balance: None,
                fee_slippage_policy: None,
            },
            dedupe_retention_ns: 500,
        }
    }

    fn test_capital_admission_runtime_feed_config() -> CapitalAdmissionRuntimeFeedConfig {
        CapitalAdmissionRuntimeFeedConfig {
            venue_id: "VENUE-A".to_string(),
            account_id: AccountId::from("ACCOUNT-001"),
            collateral_currency: "USD".to_string(),
            product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
                PredictionMarketAdmissionSnapshot {
                    source: "bolt_configured_binary_product".to_string(),
                    observed_at_ns: 900,
                    yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                    no_instrument_id: "instrument-no.VENUE-A".to_string(),
                    yes_position: Decimal::ZERO,
                    no_position: Decimal::ZERO,
                    collateral_allowance: Decimal::ZERO,
                    conditional_token_allowance: Decimal::ZERO,
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            startup_observed_at_ns: 900,
            dedupe_retention_ns: 500,
        }
    }

    fn test_venue_truth_divergence() -> VenueTruthDivergence {
        VenueTruthDivergence {
            kind: VenueTruthDivergenceKind::UnexplainedCollateralDelta,
            alarm_class: VenueTruthDivergenceAlarmClass::TrueDivergence,
            previous_captured_at: Some(UnixNanos::from(1_000)),
            current_captured_at: UnixNanos::from(1_200),
            account_id: "ACCOUNT-001".to_string(),
            field: "collateral_balance".to_string(),
            venue_value: "75".to_string(),
            prior_accepted_value: "100".to_string(),
            missing_explanation: "no filled event explains collateral delta".to_string(),
        }
    }

    fn test_venue_truth_snapshot() -> VenueTruthSnapshot {
        let currency = Currency::from("USD");
        VenueTruthSnapshot {
            captured_at: UnixNanos::from(1_200),
            account_id: AccountId::from("ACCOUNT-001"),
            collateral_balance: Money::new(50.0, currency),
            collateral_allowance: Money::new(50.0, currency),
            open_orders: BTreeMap::new(),
            positions_by_product_id: BTreeMap::new(),
        }
    }

    struct TestVenueTruthDivergenceEvidenceWriter {
        records: Mutex<Vec<VenueTruthDivergenceEvidence>>,
        fail: bool,
    }

    impl TestVenueTruthDivergenceEvidenceWriter {
        fn recording() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
                fail: true,
            }
        }

        fn records(&self) -> Vec<VenueTruthDivergenceEvidence> {
            self.records
                .lock()
                .expect("test venue truth divergence records mutex should not be poisoned")
                .clone()
        }
    }

    impl std::fmt::Debug for TestVenueTruthDivergenceEvidenceWriter {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("TestVenueTruthDivergenceEvidenceWriter")
                .field("fail", &self.fail)
                .finish_non_exhaustive()
        }
    }

    impl BoltV3DecisionEvidenceWriter for TestVenueTruthDivergenceEvidenceWriter {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
            Ok(())
        }

        fn record_admission_decision(
            &self,
            _decision: &BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_basket_admission_decision(
            &self,
            _decision: &BoltV3BasketAdmissionDecisionEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_capital_admission_rebuild_audit(
            &self,
            _audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_metadata(
            &self,
            _metadata: &BoltV3SubmitReservationMetadataEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_fill(
            &self,
            _fill: &BoltV3SubmitReservationFillEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> Result<()> {
            Ok(())
        }

        fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
            Ok(())
        }

        fn record_exit_evaluation(&self, _evidence: &BoltV3ExitEvaluationEvidence) -> Result<()> {
            Ok(())
        }

        fn record_loss_governor_halt(
            &self,
            _evidence: &BoltV3LossGovernorHaltEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_reject(&self, _evidence: &BoltV3OrderRejectEvidence) -> Result<()> {
            Ok(())
        }

        fn record_requote_throttle(&self, _throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
            Ok(())
        }

        fn record_venue_truth_divergence(
            &self,
            evidence: &VenueTruthDivergenceEvidence,
        ) -> Result<()> {
            if self.fail {
                return Err(anyhow::anyhow!("decision evidence unavailable"));
            }
            self.records
                .lock()
                .expect("test venue truth divergence records mutex should not be poisoned")
                .push(evidence.clone());
            Ok(())
        }

        fn drain_shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingTradingStateController {
        enter_reducing_calls: RefCell<usize>,
    }

    impl RecordingTradingStateController {
        fn enter_reducing_calls(&self) -> usize {
            *self.enter_reducing_calls.borrow()
        }
    }

    impl TradingStateController for RecordingTradingStateController {
        fn enter_reducing(&self) {
            *self.enter_reducing_calls.borrow_mut() += 1;
        }
    }

    struct RecordingFlattenExecutor {
        actions: RefCell<Vec<KillSwitchLossAction>>,
        fail: bool,
    }

    impl RecordingFlattenExecutor {
        fn recording() -> Self {
            Self {
                actions: RefCell::new(Vec::new()),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                actions: RefCell::new(Vec::new()),
                fail: true,
            }
        }

        fn actions(&self) -> Vec<KillSwitchLossAction> {
            self.actions.borrow().clone()
        }
    }

    impl KillSwitchFlattenExecutor for RecordingFlattenExecutor {
        fn execute_flatten(&self, action: &KillSwitchLossAction) -> Result<()> {
            self.actions.borrow_mut().push(action.clone());
            if self.fail {
                anyhow::bail!("synthetic flatten executor failure");
            }
            Ok(())
        }
    }

    fn two_command_flatten_plan(halt_id: &str) -> BoltV3KillSwitchFlattenPlan {
        let claim = BoltV3KillSwitchForcedReductionClaim::new(
            halt_id.to_string(),
            "flatten-positions",
            "a".repeat(64),
        )
        .expect("forced reduction claim should be valid");
        let first = flatten_candidate("POSITION-001");
        let second = flatten_candidate("POSITION-002");
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(BoltV3KillSwitchFlattenPlanRequest {
            kill_switch_state: KillSwitchState::Flattening {
                halt_id: halt_id.to_string(),
            },
            nt_trading_state: TradingState::Reducing,
            action_id: "flatten-positions".to_string(),
            config_sha256: "b".repeat(64),
            policy_sha256: "a".repeat(64),
            source_timestamp_unix_nanos: 2,
            policy: BoltV3KillSwitchFlattenPolicy::new(),
            snapshot: BoltV3KillSwitchFlattenSnapshot::new(vec![first, second])
                .expect("flatten snapshot should be valid"),
            observed_at_unix_nanos: 2,
            route_proof: BoltV3KillSwitchFlattenRouteProof::new(
                BoltV3KillSwitchFlattenRouteKind::LiveNodeCommandRouter,
            ),
            order_template: flatten_market_template(),
            forced_reduction_claim: claim,
        })
        .expect("two open positions should produce commands")
    }

    fn flatten_candidate(position_id: &str) -> BoltV3KillSwitchFlattenCandidate {
        BoltV3KillSwitchFlattenCandidate::from_nt_position_state(
            BoltV3KillSwitchFlattenPositionState {
                evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
                account_id: AccountId::from("ACCOUNT-001"),
                instrument_id: InstrumentId::from("instrument-yes.VENUE-A"),
                strategy_id: StrategyId::from("strategy-a"),
                position_id: PositionId::from(position_id),
                position_side: PositionSide::Long,
                quantity: Quantity::new(5.0, 2),
                source_timestamp_unix_nanos: 1,
            },
        )
        .expect("flatten candidate should be valid")
    }

    struct RoutingFlattenExecutor {
        writer: Arc<RecordingFlattenDecisionEvidenceWriter>,
        admission: Arc<BoltV3SubmitAdmissionState>,
        submitted_quantities: Rc<RefCell<Vec<Quantity>>>,
    }

    impl RoutingFlattenExecutor {
        fn new(
            writer: Arc<RecordingFlattenDecisionEvidenceWriter>,
            venue_position: Decimal,
        ) -> Self {
            let admission = flatten_admission_with_yes_position(writer.clone(), venue_position);
            admission.configure_kill_switch_forced_reduction_policy(
                BoltV3KillSwitchForcedReductionPolicy::new("a".repeat(64), 2, Decimal::new(10, 0))
                    .expect("forced reduction policy should be valid"),
            );
            Self {
                writer,
                admission,
                submitted_quantities: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn submitted_quantities(&self) -> Vec<Quantity> {
            self.submitted_quantities.borrow().clone()
        }
    }

    impl KillSwitchFlattenExecutor for RoutingFlattenExecutor {
        fn execute_flatten(&self, action: &KillSwitchLossAction) -> Result<()> {
            self.admission
                .replace_kill_switch_state(KillSwitchState::Flattening {
                    halt_id: action.halt_id.clone(),
                });
            let claim = BoltV3KillSwitchForcedReductionClaim::new(
                action.halt_id.clone(),
                action.action_id.clone(),
                "a".repeat(64),
            )
            .expect("forced reduction claim should be valid");
            let candidate = BoltV3KillSwitchFlattenCandidate::from_nt_position_state(
                BoltV3KillSwitchFlattenPositionState {
                    evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
                    account_id: AccountId::from(action.account_ids[0].as_str()),
                    instrument_id: InstrumentId::from(action.instrument_ids[0].as_str()),
                    strategy_id: StrategyId::from("strategy-a"),
                    position_id: PositionId::from("POSITION-001"),
                    position_side: PositionSide::Long,
                    quantity: Quantity::new(5.0, 2),
                    source_timestamp_unix_nanos: 1,
                },
            )
            .expect("flatten candidate should be valid");
            let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(
                BoltV3KillSwitchFlattenPlanRequest {
                    kill_switch_state: KillSwitchState::Flattening {
                        halt_id: action.halt_id.clone(),
                    },
                    nt_trading_state: TradingState::Reducing,
                    action_id: action.action_id.clone(),
                    config_sha256: "b".repeat(64),
                    policy_sha256: "a".repeat(64),
                    source_timestamp_unix_nanos: 2,
                    policy: BoltV3KillSwitchFlattenPolicy::new(),
                    snapshot: BoltV3KillSwitchFlattenSnapshot::new(vec![candidate])
                        .expect("flatten snapshot should be valid"),
                    observed_at_unix_nanos: 2,
                    route_proof: BoltV3KillSwitchFlattenRouteProof::new(
                        BoltV3KillSwitchFlattenRouteKind::LiveNodeCommandRouter,
                    ),
                    order_template: flatten_market_template(),
                    forced_reduction_claim: claim,
                },
            )
            .expect("flatten plan should produce commands");
            let command = plan
                .commands()
                .first()
                .expect("open position should produce a command");
            let mut order_factory = flatten_order_factory(command.strategy_id());
            let instrument = flatten_binary_option(command.instrument_id());
            let submitted_quantities = self.submitted_quantities.clone();
            let mut sink = BoltV3NtSubmitOnlySink::new(move |order, _context| {
                submitted_quantities.borrow_mut().push(order.quantity());
                Ok(())
            });

            route_kill_switch_flatten_command_with_sink(
                BoltV3OrderExecutionPolicy::live(),
                &mut sink,
                &mut order_factory,
                self.writer.as_ref(),
                self.admission.as_ref(),
                BoltV3KillSwitchFlattenRoutingContext {
                    execution_client_id: "execution_client",
                    fallback_price: "1",
                    instrument: Some(&instrument),
                    max_fee_bps: Decimal::ZERO,
                    submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
                },
                command,
            )?;
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct RecordingFlattenDecisionEvidenceWriter {
        records: Mutex<Vec<BoltV3OrderIntentEvidence>>,
        admission_decisions: Mutex<Vec<BoltV3AdmissionDecisionEvidence>>,
    }

    impl RecordingFlattenDecisionEvidenceWriter {
        fn records(&self) -> Vec<BoltV3OrderIntentEvidence> {
            self.records
                .lock()
                .expect("flatten records mutex should not be poisoned")
                .clone()
        }

        fn admission_decisions(&self) -> Vec<BoltV3AdmissionDecisionEvidence> {
            self.admission_decisions
                .lock()
                .expect("flatten admission mutex should not be poisoned")
                .clone()
        }
    }

    impl BoltV3DecisionEvidenceWriter for RecordingFlattenDecisionEvidenceWriter {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
            self.records
                .lock()
                .expect("flatten records mutex should not be poisoned")
                .push(intent.clone());
            Ok(())
        }

        fn record_admission_decision(
            &self,
            decision: &BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
            self.admission_decisions
                .lock()
                .expect("flatten admission mutex should not be poisoned")
                .push(decision.clone());
            Ok(())
        }

        fn record_basket_admission_decision(
            &self,
            _decision: &BoltV3BasketAdmissionDecisionEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_capital_admission_rebuild_audit(
            &self,
            _audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_metadata(
            &self,
            _metadata: &BoltV3SubmitReservationMetadataEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_fill(
            &self,
            _fill: &BoltV3SubmitReservationFillEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> Result<()> {
            Ok(())
        }

        fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
            Ok(())
        }

        fn record_exit_evaluation(&self, _evidence: &BoltV3ExitEvaluationEvidence) -> Result<()> {
            Ok(())
        }

        fn record_loss_governor_halt(
            &self,
            _evidence: &BoltV3LossGovernorHaltEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_reject(&self, _evidence: &BoltV3OrderRejectEvidence) -> Result<()> {
            Ok(())
        }

        fn record_requote_throttle(&self, _throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
            Ok(())
        }

        fn drain_shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    fn flatten_admission_with_yes_position(
        writer: Arc<RecordingFlattenDecisionEvidenceWriter>,
        yes_position: Decimal,
    ) -> Arc<BoltV3SubmitAdmissionState> {
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            writer,
            flatten_capital_admission_config(),
        ));
        let mut components = flatten_capital_admission_components();
        let ProductAdmissionSnapshot::PredictionMarketBinary(product) =
            &mut components.product_state;
        product.source = POLYMARKET_VENUE_TRUTH_REST_SOURCE.to_string();
        product.yes_position = yes_position;
        admission.update_capital_admission_nt_components(components);
        let rebuild = admission.rebuild_capital_admission_open_order_reservations(Vec::new(), 1);
        assert!(rebuild.accepted);
        admission
    }

    fn flatten_capital_admission_config() -> BoltV3SubmitCapitalAdmissionConfig {
        BoltV3SubmitCapitalAdmissionConfig {
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "USD".to_string(),
            capital_pool: CapitalPoolSnapshot {
                source: "test-capital-pool".to_string(),
                observed_at_ns: 0,
                pool_id: "pool-1".to_string(),
                max_pool_liability: Decimal::new(10, 0),
                committed_liability: Decimal::ZERO,
                max_snapshot_age_ns: u64::MAX,
            },
            policy: CapitalAdmissionPolicy {
                min_remaining_pool_balance: None,
                fee_slippage_policy: Some(FeeSlippagePolicy {
                    max_fee_liability: Decimal::new(10, 2),
                    max_slippage_liability: Decimal::new(20, 2),
                }),
            },
            dedupe_retention_ns: u64::MAX,
        }
    }

    fn flatten_capital_admission_components() -> BoltV3SubmitCapitalAdmissionNtComponents {
        BoltV3SubmitCapitalAdmissionNtComponents {
            source: "nt_capital_admission_state".to_string(),
            observed_at_ns: 0,
            portfolio: PortfolioCapitalAdmissionSnapshot {
                source: "nt_portfolio_snapshot".to_string(),
                observed_at_ns: 0,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                collateral_currency: "USD".to_string(),
                free_collateral: Decimal::new(100, 0),
                total_equity: Decimal::new(100, 0),
            },
            venue_spendability: VenueSpendabilitySnapshot {
                source: "nt_account_free_collateral".to_string(),
                observed_at_ns: 0,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                collateral_currency: "USD".to_string(),
                spendable_collateral: Decimal::new(100, 0),
                collateral_allowance: Decimal::new(100, 0),
            },
            order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
                source: "nt_open_order_cache".to_string(),
                observed_at_ns: 0,
                open_order_count: 0,
                all_open_orders_attributed: true,
            },
            product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
                PredictionMarketAdmissionSnapshot {
                    source: "nt_prediction_market_snapshot".to_string(),
                    observed_at_ns: 0,
                    yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                    no_instrument_id: "instrument-no.VENUE-A".to_string(),
                    yes_position: Decimal::ZERO,
                    no_position: Decimal::ZERO,
                    collateral_allowance: Decimal::new(100, 0),
                    conditional_token_allowance: Decimal::new(100, 0),
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            loss_snapshot: None,
        }
    }

    fn flatten_market_template() -> NtOrderTemplate {
        NtOrderTemplate {
            order_type: OrderType::Market,
            time_in_force: TimeInForce::Ioc,
            expire_time: None,
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            is_post_only: false,
            is_reduce_only: true,
            is_quote_quantity: false,
        }
    }

    fn flatten_order_factory(strategy_id: StrategyId) -> OrderFactory {
        let clock: Rc<RefCell<dyn Clock>> = Rc::new(RefCell::new(TestClock::new()));
        OrderFactory::new(
            TraderId::new("TRADER-001"),
            strategy_id,
            None,
            None,
            clock,
            false,
            true,
        )
    }

    fn flatten_binary_option(instrument_id: InstrumentId) -> InstrumentAny {
        InstrumentAny::BinaryOption(BinaryOption::new(
            instrument_id,
            Symbol::from("instrument-yes"),
            AssetClass::Alternative,
            Currency::USD(),
            UnixNanos::from(1_u64),
            UnixNanos::from(2_u64),
            2,
            2,
            Price::from("0.01"),
            Quantity::from("0.01"),
            Some(Ustr::from("YES")),
            None,
            None,
            Some(Quantity::from("0.01")),
            None,
            None,
            Some(Price::from("1.00")),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            UnixNanos::from(1_u64),
            UnixNanos::from(1_u64),
        ))
    }

    fn test_flatten_action(halt_id: &str) -> KillSwitchLossAction {
        KillSwitchLossAction {
            kind: KillSwitchLossActionKind::FlattenPositions,
            halt_id: halt_id.to_string(),
            action_id: "flatten-positions".to_string(),
            account_ids: vec!["ACCOUNT-001".to_string()],
            instrument_ids: vec!["instrument-yes.VENUE-A".to_string()],
        }
    }
}
