use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::Notify;

use crate::bolt_v3_operator_health::BoltV3OperatorHealthTransitionEmitter;
use crate::bolt_v3_venue_truth::{
    VenueTruthCaptureFailureEvidence, venue_truth_capture_failure_parts,
};

use super::*;

const OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_FAILURE: &str =
    stringify!(venue_truth_capture_failure);
const OPERATOR_HEALTH_REASON_VENUE_TRUTH_CAPTURE_RECOVERY: &str =
    stringify!(venue_truth_capture_recovery);
const OPERATOR_HEALTH_REASON_VENUE_TRUTH_RUNTIME_FAILURE: &str =
    stringify!(venue_truth_runtime_failure);
const OPERATOR_HEALTH_REASON_VENUE_TRUTH_DIVERGENCE: &str = stringify!(venue_truth_divergence);

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

impl Drop for BoltV3VenueTruthRuntimeGuard {
    fn drop(&mut self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        self.shutdown_notify.notify_waiters();
        if let Some(handle) = self.handle.take()
            && let Err(error) = handle.join()
        {
            log::error!("venue truth runtime thread join failed: {error:?}");
        }
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

/// Hard kill-switch loss-action sink that drives the NT runtime on a durable
/// daily-realized loss breach.
///
/// On a fresh `FlattenPositions` halt it moves the NT risk engine to
/// `Reducing` (venue-neutral). Active market exit is intentionally not wired:
/// current source-fence policy forbids NT `ExitMarket`/market-exit control
/// paths because they bypass Bolt's submit/cancel chokepoints. Config
/// validation rejects `flatten_open_positions_on_breach = true` until a shared
/// execution-policy flatten path exists.
struct NtReducingLossActionSink {
    trading_state: Rc<dyn TradingStateController>,
    dispatched_halts: RefCell<BTreeSet<String>>,
}

impl NtReducingLossActionSink {
    fn new(trading_state: Rc<dyn TradingStateController>) -> Self {
        Self {
            trading_state,
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
        // `Reducing` is the whole live action in this slice. Venue-mutating
        // market exit must stay out until it can be routed through a shared
        // execution-policy boundary.
        self.trading_state.enter_reducing();
        self.dispatched_halts
            .borrow_mut()
            .insert(action.halt_id.clone());
        Ok(())
    }
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
    if kill_switch.flatten_open_positions_on_breach {
        return Err(BoltV3LiveNodeError::KillSwitchLossProtection(
            anyhow::anyhow!(
                "risk.kill_switch.flatten_open_positions_on_breach=true is not supported until a shared execution-policy flatten path exists"
            ),
        ));
    }
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
    let action_sink = Rc::new(NtReducingLossActionSink::new(trading_state));
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
}

impl Drop for BoltV3LossProtectionRuntimeGuards {
    fn drop(&mut self) {
        if let Some(position_events) = self.position_events.take() {
            unsubscribe_position_events(position_events_pattern(), &position_events);
        }
        if let Some(retry_handle) = self.retry_handle.take() {
            retry_handle.abort();
        }
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
        collections::BTreeMap,
        panic::{AssertUnwindSafe, catch_unwind},
        sync::Mutex,
    };

    use super::*;

    use crate::{
        bolt_v3_capital_admission::{
            CapitalAdmissionPolicy, PredictionMarketAdmissionSnapshot, ProductAdmissionSnapshot,
            ProductKind,
        },
        bolt_v3_capital_admission_runtime_feed::{
            CapitalAdmissionRuntimeFeed, CapitalAdmissionRuntimeFeedConfig,
        },
        bolt_v3_capital_reservation::CapitalPoolSnapshot,
        bolt_v3_decision_evidence::{
            BoltV3AdmissionDecisionEvidence, BoltV3BasketAdmissionDecisionEvidence,
            BoltV3CapitalAdmissionRebuildAuditEvidence, BoltV3DecisionEvidenceWriter,
            BoltV3EntrySkipEvidence, BoltV3ExitDecisionEvidence, BoltV3ExitEvaluationEvidence,
            BoltV3LossGovernorHaltEvidence, BoltV3OrderIntentEvidence, BoltV3OrderRejectEvidence,
            BoltV3RequoteThrottleEvidence, BoltV3StrategyInputEvidenceSnapshot,
            BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
        },
        bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState, KillSwitchStateKind},
        bolt_v3_kill_switch_store::{KillSwitchRecoveryState, KillSwitchStore},
        bolt_v3_submit_admission::{
            BoltV3SubmitAdmissionState, BoltV3SubmitCapitalAdmissionConfig,
        },
        bolt_v3_venue_truth::{
            VenueTruthCaptureEndpointError, VenueTruthDivergence, VenueTruthDivergenceAlarmClass,
            VenueTruthDivergenceEvidence, VenueTruthDivergenceKind, VenueTruthSnapshot,
        },
    };
    use anyhow::Result;
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        identifiers::AccountId,
        types::{Currency, Money},
    };

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
}
