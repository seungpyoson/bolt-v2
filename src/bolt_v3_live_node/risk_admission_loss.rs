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

use crate::bolt_v3_current_evidence::OrderExecutionEvidence;
#[cfg(test)]
use crate::bolt_v3_current_evidence::{DecisionEvidenceRecorder, DecisionEvidenceStatusView};
use crate::bolt_v3_operator_health::BoltV3OperatorHealthTransitionEmitter;
use crate::bolt_v3_provider_collateral_allowance::{
    ProviderCollateralAllowanceCaptureFailureEvidence,
    provider_collateral_allowance_capture_failure_parts,
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
    },
};

use super::*;

const OPERATOR_HEALTH_REASON_PROVIDER_COLLATERAL_ALLOWANCE_CAPTURE_FAILURE: &str =
    stringify!(provider_collateral_allowance_capture_failure);
const OPERATOR_HEALTH_REASON_PROVIDER_COLLATERAL_ALLOWANCE_RUNTIME_FAILURE: &str =
    stringify!(provider_collateral_allowance_runtime_failure);

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

pub(super) struct BoltV3ProviderCollateralAllowanceRuntimeConfig {
    pub(super) source: Arc<
        dyn crate::bolt_v3_provider_collateral_allowance::ProviderCollateralAllowanceSnapshotSource,
    >,
    pub(super) poll_interval_ms: u64,
}

pub(super) struct BoltV3ProviderCollateralAllowanceRuntimeGuard {
    shutdown_requested: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl BoltV3ProviderCollateralAllowanceRuntimeGuard {
    pub(super) fn stop_and_join(mut self) {
        self.stop_and_join_inner();
    }

    fn stop_and_join_inner(&mut self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        self.shutdown_notify.notify_waiters();
        if let Some(handle) = self.handle.take()
            && let Err(error) = handle.join()
        {
            log::error!("provider collateral allowance runtime thread join failed: {error:?}");
        }
    }
}

impl Drop for BoltV3ProviderCollateralAllowanceRuntimeGuard {
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

pub(super) fn provider_collateral_allowance_runtime_config_from_loaded(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    feed_config: Option<&CapitalAdmissionRuntimeFeedConfig>,
) -> Result<Option<BoltV3ProviderCollateralAllowanceRuntimeConfig>, BoltV3LiveNodeError> {
    let Some(feed_config) = feed_config else {
        return Ok(None);
    };
    let Some(binding) = crate::bolt_v3_providers::binding_for_provider_key(&feed_config.venue_id)
    else {
        return Ok(None);
    };
    let Some(build_source) = binding.build_provider_collateral_allowance_runtime_source else {
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
                "capital admission requires a configured execution client for provider collateral allowance on venue `{}`",
                feed_config.venue_id
            )));
        }
        [(client_key, client)] => (client_key.as_str(), *client),
        _ => {
            return Err(BoltV3LiveNodeError::Build(anyhow::anyhow!(
                "capital admission requires one execution client for provider collateral allowance on venue `{}`; found {}",
                feed_config.venue_id,
                matching_clients
                    .iter()
                    .map(|(client_key, _)| client_key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    };
    let source = build_source(
        crate::bolt_v3_providers::ProviderCollateralAllowanceSourceContext {
            client_key,
            client,
            resolved,
            collateral_currency: feed_config.collateral_currency.as_str(),
        },
    )
    .map_err(BoltV3LiveNodeError::Build)?;

    Ok(Some(BoltV3ProviderCollateralAllowanceRuntimeConfig {
        source: source.source,
        poll_interval_ms: source.poll_interval_ms,
    }))
}

pub(super) fn spawn_provider_collateral_allowance_runtime(
    config: BoltV3ProviderCollateralAllowanceRuntimeConfig,
    feed: Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    stop_handle: LiveNodeHandle,
    health_emitter: Option<BoltV3OperatorHealthTransitionEmitter>,
    nt_projection_requested: Arc<AtomicBool>,
) -> BoltV3ProviderCollateralAllowanceRuntimeGuard {
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let shutdown_notify = Arc::new(Notify::new());
    let thread_shutdown_requested = Arc::clone(&shutdown_requested);
    let thread_shutdown_notify = Arc::clone(&shutdown_notify);
    let spawn_submit_admission = Arc::clone(&submit_admission);
    let spawn_stop_handle = stop_handle.clone();
    let spawn_health_emitter = health_emitter.clone();
    let handle = std::thread::Builder::new()
        .name("bolt-v3-provider-allowance-runtime".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    halt_for_provider_collateral_allowance(
                        &spawn_submit_admission,
                        &spawn_stop_handle,
                        0,
                        format!("provider collateral allowance runtime build failed: {error:#}"),
                        spawn_health_emitter.as_ref(),
                    );
                    return;
                }
            };
            runtime.block_on(run_provider_collateral_allowance_runtime(
                config,
                ProviderCollateralAllowanceRuntimeContext {
                    feed,
                    submit_admission: spawn_submit_admission,
                    stop_handle: spawn_stop_handle,
                    shutdown_requested: thread_shutdown_requested,
                    shutdown_notify: thread_shutdown_notify,
                    health_emitter: spawn_health_emitter,
                    nt_projection_requested,
                },
            ));
        });
    let handle = match handle {
        Ok(handle) => Some(handle),
        Err(error) => {
            halt_for_provider_collateral_allowance(
                &submit_admission,
                &stop_handle,
                0,
                format!("provider collateral allowance runtime thread spawn failed: {error:#}"),
                health_emitter.as_ref(),
            );
            None
        }
    };
    BoltV3ProviderCollateralAllowanceRuntimeGuard {
        shutdown_requested,
        shutdown_notify,
        handle,
    }
}

struct ProviderCollateralAllowanceRuntimeContext {
    feed: Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    stop_handle: LiveNodeHandle,
    shutdown_requested: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
    health_emitter: Option<BoltV3OperatorHealthTransitionEmitter>,
    nt_projection_requested: Arc<AtomicBool>,
}

async fn run_provider_collateral_allowance_runtime(
    config: BoltV3ProviderCollateralAllowanceRuntimeConfig,
    context: ProviderCollateralAllowanceRuntimeContext,
) {
    let ProviderCollateralAllowanceRuntimeContext {
        feed,
        submit_admission,
        stop_handle,
        shutdown_requested,
        shutdown_notify,
        health_emitter,
        nt_projection_requested,
    } = context;
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
                halt_for_provider_collateral_allowance(
                    &submit_admission,
                    &stop_handle,
                    0,
                    format!("clock failed before provider collateral allowance poll: {error:#}"),
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
                handle_provider_collateral_allowance_capture_failure(
                    &submit_admission,
                    captured_at,
                    captures_missed,
                    &error,
                );
                if let Some(health_emitter) = health_emitter.as_ref() {
                    health_emitter(
                        OPERATOR_HEALTH_REASON_PROVIDER_COLLATERAL_ALLOWANCE_CAPTURE_FAILURE,
                    );
                }
                continue;
            }
        };
        captures_missed = 0;
        record_provider_collateral_allowance_snapshot(&feed, snapshot);
        nt_projection_requested.store(true, Ordering::Release);
    }
}

fn record_provider_collateral_allowance_snapshot(
    feed: &Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
    snapshot: ProviderCollateralAllowanceSnapshot,
) {
    let mut feed = feed
        .lock()
        .expect("provider collateral allowance snapshot feed lock poisoned");
    feed.on_provider_collateral_allowance_snapshot(snapshot);
}

fn handle_provider_collateral_allowance_capture_failure(
    submit_admission: &BoltV3SubmitAdmissionState,
    observed_at_ns: u64,
    captures_missed: u64,
    error: &anyhow::Error,
) {
    log::error!("provider collateral allowance poll failed: {error:#}");
    submit_admission.suspend_capital_admission_for_provider_collateral_allowance_capture_failure(
        provider_collateral_allowance_capture_failure_evidence(
            observed_at_ns,
            captures_missed,
            error,
        ),
    );
}

fn provider_collateral_allowance_capture_failure_evidence(
    observed_at_ns: u64,
    captures_missed: u64,
    error: &anyhow::Error,
) -> ProviderCollateralAllowanceCaptureFailureEvidence {
    let (endpoint, error_class) = provider_collateral_allowance_capture_failure_parts(error);
    ProviderCollateralAllowanceCaptureFailureEvidence {
        source:
            crate::bolt_v3_capital_admission_runtime_feed::POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE
                .to_string(),
        observed_at_ns,
        endpoint,
        error_class,
        captures_missed,
    }
}

fn halt_for_provider_collateral_allowance(
    submit_admission: &BoltV3SubmitAdmissionState,
    stop_handle: &LiveNodeHandle,
    source_timestamp_unix_nanos: u64,
    reason: String,
    health_emitter: Option<&BoltV3OperatorHealthTransitionEmitter>,
) {
    let state = latch_non_durable_provider_collateral_allowance_runtime_failure(
        submit_admission,
        source_timestamp_unix_nanos,
        reason,
    );
    log::error!(
        "provider collateral allowance runtime failure latched memory-only kill switch: {:?}",
        state.kind()
    );
    if let Some(health_emitter) = health_emitter {
        health_emitter(OPERATOR_HEALTH_REASON_PROVIDER_COLLATERAL_ALLOWANCE_RUNTIME_FAILURE);
    }
    stop_handle.stop();
}

fn latch_non_durable_provider_collateral_allowance_runtime_failure(
    submit_admission: &BoltV3SubmitAdmissionState,
    source_timestamp_unix_nanos: u64,
    reason: String,
) -> KillSwitchState {
    let current = submit_admission.kill_switch_state();
    if current.kind() != KillSwitchStateKind::Armed {
        return current;
    }
    let source =
        crate::bolt_v3_capital_admission_runtime_feed::POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE;
    let trigger = KillSwitchHaltTrigger::provider_collateral_allowance_runtime_failure(
        source,
        source_timestamp_unix_nanos,
        reason.clone(),
    );
    let fallback_halt_id = crate::bolt_v3_kill_switch::halt_id_for_trigger(&trigger);
    let failed = transition_kill_switch_state(
        KillSwitchState::Armed,
        KillSwitchEvent::HaltTriggered(trigger),
        provider_collateral_allowance_kill_switch_transition_context(false, false),
    )
    .and_then(|halting| {
        transition_kill_switch_state(
            halting,
            KillSwitchEvent::HaltActionDispatchFailed { reason },
            provider_collateral_allowance_kill_switch_transition_context(false, false),
        )
    })
    .unwrap_or_else(|error| KillSwitchState::FailedManualIntervention {
        halt_id: fallback_halt_id,
        reason: format!(
            "provider collateral allowance runtime fail-closed transition failed: {error:?}"
        ),
    });
    submit_admission.replace_kill_switch_state(failed.clone());
    failed
}

fn provider_collateral_allowance_kill_switch_transition_context(
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
    let pool =
        crate::bolt_v3_settlement_runtime::capital_admission_runtime_feed_pool(&loaded.root)?;
    let product = pool
        .prediction_market_binary
        .as_ref()
        .expect("capital-admission runtime feed pool selector requires prediction_market_binary");
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
                collateral_coupled_group_id: product.collateral_coupled_group_id.clone(),
            },
        ),
    })
}

pub(super) fn order_reject_observer_account_id_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Option<AccountId> {
    let pools = loaded.root.risk.capital_pools.as_ref()?;
    let pool = pools.iter().find(|pool| pool.enforce_submit_admission)?;
    Some(pool.account_id)
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
    decision_evidence: OrderExecutionEvidence,
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
                    &decision_evidence,
                    submit_admission.as_ref(),
                    BoltV3KillSwitchFlattenRoutingContext {
                        execution_client_id,
                        fallback_price: fallback_price.as_str(),
                        instrument: Some(&instrument),
                        max_fee_bps: Decimal::ZERO,
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
    decision_evidence: impl Into<OrderExecutionEvidence>,
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
        decision_evidence.into(),
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
            POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE,
        },
        bolt_v3_capital_admission_state::{
            OrderLifecycleCapitalAdmissionSnapshot, PortfolioCapitalAdmissionSnapshot,
            ProviderCollateralAllowanceSnapshot,
        },
        bolt_v3_capital_reservation::CapitalPoolSnapshot,
        bolt_v3_kill_switch::{KillSwitchState, KillSwitchStateKind},
        bolt_v3_kill_switch_store::{KillSwitchRecoveryState, KillSwitchStore},
        bolt_v3_order_execution::{
            BoltV3KillSwitchFlattenRoutingContext, BoltV3NtSubmitOnlySink,
            BoltV3OrderExecutionPolicy, route_kill_switch_flatten_command_with_sink,
        },
        bolt_v3_order_intent::NtOrderTemplate,
        bolt_v3_provider_collateral_allowance::{
            ProviderCollateralAllowanceCaptureEndpoint,
            ProviderCollateralAllowanceCaptureEndpointError,
            ProviderCollateralAllowanceCaptureErrorClass,
        },
        bolt_v3_submit_admission::{
            BoltV3KillSwitchForcedReductionClaim, BoltV3KillSwitchForcedReductionPolicy,
            BoltV3SubmitAdmissionState, BoltV3SubmitCapitalAdmissionConfig,
            BoltV3SubmitCapitalAdmissionNtComponents,
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
        types::{Currency, Price, Quantity},
    };
    use ustr::Ustr;

    #[test]
    fn provider_collateral_allowance_capture_failure_evidence_uses_production_endpoint_error_parts()
    {
        let error = anyhow::anyhow!(ProviderCollateralAllowanceCaptureEndpointError::new(
            ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance,
            ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode,
            anyhow::anyhow!("transport failed"),
        ))
        .context("poll provider collateral allowance");

        let evidence = provider_collateral_allowance_capture_failure_evidence(1_100, 3, &error);

        assert_eq!(
            evidence.source,
            crate::bolt_v3_capital_admission_runtime_feed::POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE
        );
        assert_eq!(evidence.observed_at_ns, 1_100);
        assert_eq!(
            evidence.endpoint,
            ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance
        );
        assert_eq!(
            evidence.error_class,
            ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode
        );
        assert_eq!(evidence.captures_missed, 3);
    }

    #[test]
    fn provider_collateral_allowance_capture_failure_handler_suspends_without_durable_halt() {
        let admission = BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(DecisionEvidenceRecorder::recording()),
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
        let error = anyhow::anyhow!(ProviderCollateralAllowanceCaptureEndpointError::new(
            ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance,
            ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode,
            anyhow::anyhow!("transport failed"),
        ));

        handle_provider_collateral_allowance_capture_failure(&admission, 1_200, 2, &error);

        assert_eq!(
            admission.kill_switch_state_kind(),
            KillSwitchStateKind::Armed
        );
        assert_eq!(admission.capital_admission_reconciled(), Some(false));
    }

    #[test]
    fn provider_collateral_allowance_failure_recovery_repeat_failure_emits_three_health_transitions()
     {
        let decision_evidence = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            decision_evidence.clone(),
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
                let surface = live_operator_health_surface(
                    None,
                    &admission,
                    true,
                    0,
                    None,
                    BoltV3SettlementHealth::nominal(),
                    &DecisionEvidenceStatusView::new(&decision_evidence),
                );
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
        let mut baseline = test_provider_collateral_allowance_snapshot();
        baseline.observed_at_ns = 1_100;
        record_provider_collateral_allowance_snapshot(&feed, baseline);
        apply_empty_canonical_nt_projection(&feed, &admission, 1_150);
        assert_eq!(admission.capital_admission_reconciled(), Some(true));

        let error = anyhow::anyhow!(ProviderCollateralAllowanceCaptureEndpointError::new(
            ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance,
            ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode,
            anyhow::anyhow!("transport failed"),
        ));

        handle_provider_collateral_allowance_capture_failure(&admission, 1_200, 1, &error);
        health_emitter(OPERATOR_HEALTH_REASON_PROVIDER_COLLATERAL_ALLOWANCE_CAPTURE_FAILURE);

        let mut recovery = test_provider_collateral_allowance_snapshot();
        recovery.observed_at_ns = 1_300;
        record_provider_collateral_allowance_snapshot(&feed, recovery);
        assert_eq!(admission.capital_admission_reconciled(), Some(false));
        apply_empty_canonical_nt_projection(&feed, &admission, 1_350);
        health_emitter(OPERATOR_HEALTH_REASON_SUBMIT_ADMISSION_NT_PROJECTION);

        handle_provider_collateral_allowance_capture_failure(&admission, 1_400, 1, &error);
        health_emitter(OPERATOR_HEALTH_REASON_PROVIDER_COLLATERAL_ALLOWANCE_CAPTURE_FAILURE);

        let emissions = emissions
            .lock()
            .expect("test emissions lock should not be poisoned")
            .clone();
        assert_eq!(
            emissions,
            vec![
                OPERATOR_HEALTH_REASON_PROVIDER_COLLATERAL_ALLOWANCE_CAPTURE_FAILURE,
                OPERATOR_HEALTH_REASON_SUBMIT_ADMISSION_NT_PROJECTION,
                OPERATOR_HEALTH_REASON_PROVIDER_COLLATERAL_ALLOWANCE_CAPTURE_FAILURE,
            ]
        );
    }

    #[test]
    fn provider_collateral_allowance_runtime_failure_latches_without_writing_durable_halt() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let store = KillSwitchStore::new(temp.path().join("kill-switch.json"), 65_536);
        store
            .write_state(&KillSwitchState::Armed)
            .expect("recovered armed state should persist");
        let admission = BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(DecisionEvidenceRecorder::recording()),
            test_capital_admission_config(),
        );

        let state = latch_non_durable_provider_collateral_allowance_runtime_failure(
            &admission,
            1_300,
            "clock failed before provider collateral allowance poll".to_string(),
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
        let pre_wiring_writer = Arc::new(DecisionEvidenceRecorder::recording());
        let pre_wiring_executor = Rc::new(RoutingFlattenExecutor::new(
            pre_wiring_writer.clone(),
            Decimal::new(3, 0),
        ));
        let pre_wiring_setup_facts = pre_wiring_writer
            .recorded_facts()
            .expect("pre-wiring setup evidence should decode");
        assert!(matches!(
            pre_wiring_setup_facts.as_slice(),
            [crate::bolt_v3_current_evidence::CurrentFact::CapitalAdmissionRebuild(_)]
        ));
        let pre_wiring_trading_state = Rc::new(RecordingTradingStateController::default());
        let pre_wiring_sink = NtReducingLossActionSink::new(pre_wiring_trading_state.clone());
        pre_wiring_sink
            .emit(test_flatten_action("halt-pre-wiring"))
            .expect("pre-wiring sink should only latch NT reducing");

        assert_eq!(pre_wiring_trading_state.enter_reducing_calls(), 1);
        assert_eq!(pre_wiring_executor.submitted_quantities(), Vec::new());
        assert_eq!(
            pre_wiring_writer
                .recorded_facts()
                .expect("post-action current evidence should decode"),
            pre_wiring_setup_facts,
            "an unwired flatten action must not add decision evidence"
        );

        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let wired_executor = Rc::new(RoutingFlattenExecutor::new(
            writer.clone(),
            Decimal::new(3, 0),
        ));
        let wired_setup_fact_count = writer
            .recorded_facts()
            .expect("wired setup evidence should decode")
            .len();
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
        let facts = writer
            .recorded_facts()
            .expect("flatten current evidence should decode");
        let action_facts = &facts[wired_setup_fact_count..];
        let records = action_facts
            .iter()
            .filter_map(|fact| match fact {
                crate::bolt_v3_current_evidence::CurrentFact::RiskReducingExitOrderIntent(
                    record,
                ) => Some(record),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].details.clamp_outcome,
            Some(
                crate::bolt_v3_current_evidence::OrderIntentClampOutcome::Clamped {
                    original_quantity: Quantity::new(5.0, 2).as_decimal().to_string(),
                }
            )
        );
        let admission_decisions = action_facts
            .iter()
            .filter_map(|fact| match fact {
                crate::bolt_v3_current_evidence::CurrentFact::ForcedReductionAdmission(record) => {
                    Some(record)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(admission_decisions.len(), 1);
        assert_eq!(
            admission_decisions[0].outcome,
            crate::bolt_v3_current_evidence::AdmissionDecisionOutcome::Admitted
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
    #[should_panic(expected = "provider collateral allowance snapshot feed lock poisoned")]
    fn provider_collateral_allowance_snapshot_feed_lock_poison_panics() {
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            Arc::new(DecisionEvidenceRecorder::recording()),
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
            panic!("poison provider collateral allowance reconcile feed lock");
        }));
        assert!(poisoned.is_err());
        assert!(feed.lock().is_err());

        record_provider_collateral_allowance_snapshot(
            &feed,
            test_provider_collateral_allowance_snapshot(),
        );
    }

    fn apply_empty_canonical_nt_projection(
        feed: &Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
        admission: &BoltV3SubmitAdmissionState,
        observed_at_ns: u64,
    ) {
        let (mut components, allowance_observed_at_ns) = {
            let feed = feed
                .lock()
                .expect("provider collateral allowance snapshot feed should lock");
            let allowance_observed_at_ns = feed
                .accepted_allowance_observed_at_ns()
                .expect("provider collateral allowance must precede NT projection");
            let components = feed
                .canonical_nt_components(CapitalAdmissionNtCacheProjection {
                    accepted_allowance_observed_at_ns: Some(allowance_observed_at_ns),
                    account_balances: Some((Decimal::new(100, 0), Decimal::new(100, 0))),
                    open_client_order_ids: Vec::new(),
                    yes_position: Decimal::ZERO,
                    no_position: Decimal::ZERO,
                    observed_at_ns,
                })
                .expect("canonical empty NT projection should be complete");
            (components, allowance_observed_at_ns)
        };
        admission.update_capital_admission_nt_components(components.clone());
        let rebuild =
            admission.rebuild_capital_admission_open_order_reservations(Vec::new(), observed_at_ns);
        assert!(
            rebuild.accepted,
            "canonical empty NT projection should rebuild the reservation ledger"
        );
        components.order_lifecycle.all_open_orders_attributed = true;
        admission.update_capital_admission_nt_components_after_accepted_allowance_snapshot(
            components,
            allowance_observed_at_ns,
        );
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
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
        }
    }

    fn test_provider_collateral_allowance_snapshot() -> ProviderCollateralAllowanceSnapshot {
        ProviderCollateralAllowanceSnapshot {
            source: POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE.to_string(),
            observed_at_ns: 1_200,
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            collateral_currency: "USD".to_string(),
            collateral_allowance: Decimal::new(50, 0),
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
        writer: Arc<DecisionEvidenceRecorder>,
        admission: Arc<BoltV3SubmitAdmissionState>,
        submitted_quantities: Rc<RefCell<Vec<Quantity>>>,
    }

    impl RoutingFlattenExecutor {
        fn new(writer: Arc<DecisionEvidenceRecorder>, venue_position: Decimal) -> Self {
            let admission = flatten_admission_with_yes_position(writer.clone(), venue_position);
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
                },
                command,
            )?;
            Ok(())
        }
    }

    fn flatten_admission_with_yes_position(
        writer: Arc<DecisionEvidenceRecorder>,
        yes_position: Decimal,
    ) -> Arc<BoltV3SubmitAdmissionState> {
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            writer,
            flatten_capital_admission_config(),
        ));
        let mut components = flatten_capital_admission_components();
        let ProductAdmissionSnapshot::PredictionMarketBinary(product) =
            &mut components.product_state;
        product.source = POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE.to_string();
        product.yes_position = yes_position;
        admission.update_capital_admission_nt_components(components);
        admission.configure_kill_switch_forced_reduction_policy(
            BoltV3KillSwitchForcedReductionPolicy::new("a".repeat(64), 2, Decimal::new(10, 0))
                .expect("forced reduction policy should be valid"),
        );
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
            provider_collateral_allowance: ProviderCollateralAllowanceSnapshot {
                source: "nt_account_free_collateral".to_string(),
                observed_at_ns: 0,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                collateral_currency: "USD".to_string(),
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
