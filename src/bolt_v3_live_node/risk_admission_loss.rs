use super::*;

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
