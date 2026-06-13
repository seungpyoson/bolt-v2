use std::{collections::BTreeMap, rc::Rc, sync::Arc};

use anyhow::anyhow;
use nautilus_model::{events::PositionEvent, types::Money};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_kill_switch::{
        KillSwitchEvent, KillSwitchHaltTrigger, KillSwitchState, KillSwitchTransitionContext,
        transition_kill_switch_state,
    },
    bolt_v3_kill_switch_store::{
        KillSwitchCumulativePositionPnlSnapshot, KillSwitchLossProtectionSnapshot,
        KillSwitchPendingHaltActionsSnapshot, KillSwitchRecoveryReason, KillSwitchRecoveryState,
        KillSwitchStore,
    },
    bolt_v3_submit_admission::{BoltV3KillSwitchForcedReductionPolicy, BoltV3SubmitAdmissionState},
};

const NANOS_PER_UTC_DAY: u64 = 86_400_000_000_000;
const NANOS_PER_MILLISECOND: u64 = 1_000_000;
const FAIL_CLOSED_RECOVERY_HALT_ID: &str = "kill-switch-recovery-fail-closed";
const LOSS_TRIGGER_REASON: &str = "daily_realized_loss_limit";
const HALT_ACTION_RETRY_TIMEOUT_REASON: &str = "halt_action_retry_timeout";
const FLATTEN_ACTION_ID: &str = "flatten-positions";
const SNAPSHOT_PERSISTENCE_FAILED_REASON: &str = "loss_protection_snapshot_persistence_failed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillSwitchLossProtectionConfig {
    pub daily_realized_loss_limit: Decimal,
    pub action_retry_interval_ms: u64,
    pub action_retry_timeout_ms: u64,
    pub forced_reduction_policy: BoltV3KillSwitchForcedReductionPolicy,
    pub policy_sha256: String,
    pub account_ids: Vec<String>,
    pub instrument_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealizedPnlObservation {
    pub source: &'static str,
    pub observed_at_unix_nanos: u64,
    pub realized_pnl: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionRealizedPnlObservation {
    pub account_id: String,
    pub instrument_id: String,
    pub position_id: String,
    pub observed: RealizedPnlObservation,
    pub cumulative_realized_pnl: bool,
    pub closes_position: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSwitchLossActionKind {
    FlattenPositions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillSwitchLossAction {
    pub kind: KillSwitchLossActionKind,
    pub halt_id: String,
    pub action_id: String,
    pub policy_sha256: String,
    pub account_ids: Vec<String>,
    pub instrument_ids: Vec<String>,
}

pub trait KillSwitchLossActionSink {
    fn emit(&self, action: KillSwitchLossAction) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingHaltActions {
    state: KillSwitchState,
    next_retry_at_unix_nanos: u64,
    retry_deadline_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CumulativePositionPnl {
    realized_pnl: Decimal,
    last_observed_at_unix_nanos: u64,
}

pub struct KillSwitchLossProtection {
    config: KillSwitchLossProtectionConfig,
    admission: Arc<BoltV3SubmitAdmissionState>,
    store: KillSwitchStore,
    action_sink: Rc<dyn KillSwitchLossActionSink>,
    state: KillSwitchState,
    daily_bucket: Option<u64>,
    daily_realized_pnl: Decimal,
    cumulative_position_pnl: BTreeMap<String, CumulativePositionPnl>,
    pending_halt_actions: Option<PendingHaltActions>,
}

impl KillSwitchLossProtection {
    pub fn new(
        config: KillSwitchLossProtectionConfig,
        admission: Arc<BoltV3SubmitAdmissionState>,
        store: KillSwitchStore,
        action_sink: Rc<dyn KillSwitchLossActionSink>,
    ) -> anyhow::Result<Self> {
        admission
            .configure_kill_switch_forced_reduction_policy(config.forced_reduction_policy.clone());
        Ok(Self {
            config,
            admission,
            store,
            action_sink,
            state: KillSwitchState::Armed,
            daily_bucket: None,
            daily_realized_pnl: Decimal::ZERO,
            cumulative_position_pnl: BTreeMap::new(),
            pending_halt_actions: None,
        })
    }

    pub fn store(&self) -> &KillSwitchStore {
        &self.store
    }

    pub fn seed_from_store(&mut self) -> anyhow::Result<KillSwitchState> {
        let record = self
            .store
            .load_recovery_record()
            .map_err(|error| anyhow!("kill switch recovery load failed: {error:?}"))?;
        let mut state = match record.recovery_state {
            KillSwitchRecoveryState::Recovered(state) => state,
            KillSwitchRecoveryState::FailClosed { reason, state } => {
                fail_closed_recovery_state(reason, state)
            }
        };
        self.state = state.clone();
        if matches!(state, KillSwitchState::Armed) {
            let Some(snapshot) = record.loss_protection else {
                state = fail_closed_recovery_state(
                    KillSwitchRecoveryReason::MissingLossProtectionSnapshot,
                    Some(state),
                );
                self.admission.replace_kill_switch_state(state.clone());
                self.state = state.clone();
                self.persist_failed_state_or_invalidate(&state)?;
                return Ok(state);
            };
            self.apply_loss_snapshot(snapshot);
        } else if let Some(snapshot) = record.loss_protection {
            self.apply_loss_snapshot(snapshot);
        }
        self.admission.replace_kill_switch_state(state.clone());
        self.state = state.clone();
        Ok(state)
    }

    pub fn action_retry_interval_ms(&self) -> u64 {
        self.config.action_retry_interval_ms
    }

    pub fn record_position_event(
        &mut self,
        event: &PositionEvent,
    ) -> anyhow::Result<Option<KillSwitchState>> {
        let Some(observation) = position_realized_pnl_observation(event) else {
            return Ok(None);
        };
        if !self
            .config
            .account_ids
            .iter()
            .any(|account_id| account_id == &observation.account_id)
            || !self
                .config
                .instrument_ids
                .iter()
                .any(|instrument_id| instrument_id == &observation.instrument_id)
        {
            return Ok(None);
        }
        if !matches!(self.state, KillSwitchState::Armed) {
            self.poll_pending_halt_actions(observation.observed.observed_at_unix_nanos)?;
            return Ok(None);
        }
        if !self.accept_observation_bucket(observation.observed.observed_at_unix_nanos) {
            return Ok(None);
        }
        let observed = if observation.cumulative_realized_pnl {
            let previous_record = self
                .cumulative_position_pnl
                .get(&observation.position_id)
                .cloned();
            if let Some(previous) = &previous_record {
                if observation.observed.observed_at_unix_nanos
                    < previous.last_observed_at_unix_nanos
                {
                    return Ok(None);
                }
                if observation.observed.observed_at_unix_nanos
                    == previous.last_observed_at_unix_nanos
                    && observation.observed.realized_pnl == previous.realized_pnl
                {
                    if observation.closes_position {
                        self.cumulative_position_pnl
                            .remove(&observation.position_id);
                        self.persist_runtime_snapshot_or_fail_closed()?;
                    }
                    return Ok(None);
                }
            }
            let previous = previous_record
                .map(|previous| previous.realized_pnl)
                .unwrap_or(Decimal::ZERO);
            if observation.closes_position {
                self.cumulative_position_pnl
                    .remove(&observation.position_id);
            } else {
                self.cumulative_position_pnl.insert(
                    observation.position_id.clone(),
                    CumulativePositionPnl {
                        realized_pnl: observation.observed.realized_pnl,
                        last_observed_at_unix_nanos: observation.observed.observed_at_unix_nanos,
                    },
                );
            }
            RealizedPnlObservation {
                realized_pnl: observation.observed.realized_pnl - previous,
                ..observation.observed
            }
        } else {
            observation.observed
        };
        self.record_current_bucket_realized_pnl(observed)
    }

    pub fn record_realized_pnl(
        &mut self,
        observation: RealizedPnlObservation,
    ) -> anyhow::Result<Option<KillSwitchState>> {
        if !matches!(self.state, KillSwitchState::Armed) {
            self.poll_pending_halt_actions(observation.observed_at_unix_nanos)?;
            return Ok(None);
        }

        if !self.accept_observation_bucket(observation.observed_at_unix_nanos) {
            return Ok(None);
        }
        self.record_current_bucket_realized_pnl(observation)
    }

    fn record_current_bucket_realized_pnl(
        &mut self,
        observation: RealizedPnlObservation,
    ) -> anyhow::Result<Option<KillSwitchState>> {
        self.daily_realized_pnl += observation.realized_pnl;

        if self.daily_realized_pnl >= Decimal::ZERO
            || -self.daily_realized_pnl < self.config.daily_realized_loss_limit
        {
            self.persist_runtime_snapshot_or_fail_closed()?;
            return Ok(None);
        }

        let trigger = KillSwitchHaltTrigger::loss_governor_breach(
            observation.source,
            observation.observed_at_unix_nanos,
            LOSS_TRIGGER_REASON,
        );
        let halting = transition_kill_switch_state(
            KillSwitchState::Armed,
            KillSwitchEvent::HaltTriggered(trigger),
            inert_transition_context(),
        )
        .map_err(|error| anyhow!("daily realized loss transition failed: {error:?}"))?;

        if let Err(error) = self
            .store
            .write_state_with_loss_snapshot(&halting, Some(&self.loss_snapshot()))
        {
            let reason = format!("daily realized loss halt persistence failed: {error:?}");
            let failed = transition_kill_switch_state(
                halting,
                KillSwitchEvent::DurableHaltEvidenceWriteFailed {
                    reason: reason.clone(),
                },
                inert_transition_context(),
            )
            .map_err(|error| {
                anyhow!("daily realized loss fail-closed transition failed: {error:?}")
            })?;
            self.admission.replace_kill_switch_state(failed.clone());
            self.state = failed.clone();
            self.persist_failed_state_or_invalidate(&failed)
                .map_err(|persist_error| anyhow!("{reason}; {persist_error}"))?;
            return Err(anyhow!(reason));
        }
        self.admission.replace_kill_switch_state(halting.clone());
        self.state = halting.clone();
        if let Err(error) = self.emit_halt_actions(&halting) {
            self.pending_halt_actions = Some(PendingHaltActions {
                state: halting,
                next_retry_at_unix_nanos: add_millis(
                    observation.observed_at_unix_nanos,
                    self.config.action_retry_interval_ms,
                ),
                retry_deadline_unix_nanos: add_millis(
                    observation.observed_at_unix_nanos,
                    self.config.action_retry_timeout_ms,
                ),
            });
            self.persist_runtime_snapshot_or_fail_closed()?;
            return Err(anyhow!(
                "daily realized loss halt action dispatch failed: {error:?}"
            ));
        }
        Ok(Some(halting))
    }

    fn accept_observation_bucket(&mut self, observed_at_unix_nanos: u64) -> bool {
        let bucket = observed_at_unix_nanos / NANOS_PER_UTC_DAY;
        match self.daily_bucket {
            None => {
                self.daily_bucket = Some(bucket);
                self.daily_realized_pnl = Decimal::ZERO;
                true
            }
            Some(current) if bucket > current => {
                self.daily_bucket = Some(bucket);
                self.daily_realized_pnl = Decimal::ZERO;
                true
            }
            Some(current) if bucket == current => true,
            Some(_) => false,
        }
    }

    pub fn poll_pending_halt_actions(&mut self, observed_at_unix_nanos: u64) -> anyhow::Result<()> {
        let Some(pending) = self.pending_halt_actions.clone() else {
            return Ok(());
        };
        if observed_at_unix_nanos < pending.next_retry_at_unix_nanos {
            return Ok(());
        }
        if observed_at_unix_nanos > pending.retry_deadline_unix_nanos {
            self.fail_halt_actions(pending.state, HALT_ACTION_RETRY_TIMEOUT_REASON.to_string())?;
            self.pending_halt_actions = None;
            return Err(anyhow!("daily realized loss halt action retry timeout"));
        }

        if let Err(error) = self.emit_halt_actions(&pending.state) {
            self.pending_halt_actions = Some(PendingHaltActions {
                state: pending.state,
                next_retry_at_unix_nanos: add_millis(
                    observed_at_unix_nanos,
                    self.config.action_retry_interval_ms,
                ),
                retry_deadline_unix_nanos: pending.retry_deadline_unix_nanos,
            });
            self.persist_runtime_snapshot_or_fail_closed()?;
            return Err(anyhow!(
                "daily realized loss halt action retry failed: {error:?}"
            ));
        }
        self.pending_halt_actions = None;
        self.persist_runtime_snapshot_or_fail_closed()?;
        Ok(())
    }

    fn fail_halt_actions(&mut self, state: KillSwitchState, reason: String) -> anyhow::Result<()> {
        let failed = transition_kill_switch_state(
            state,
            KillSwitchEvent::HaltActionDispatchFailed { reason },
            inert_transition_context(),
        )
        .map_err(|error| anyhow!("halt action failure transition failed: {error:?}"))?;
        self.admission.replace_kill_switch_state(failed.clone());
        self.state = failed.clone();
        self.persist_failed_state_or_invalidate(&failed)?;
        Ok(())
    }

    fn emit_halt_actions(&self, state: &KillSwitchState) -> anyhow::Result<()> {
        let Some(halt_id) = halt_id(state) else {
            return Ok(());
        };
        self.action_sink.emit(KillSwitchLossAction {
            kind: KillSwitchLossActionKind::FlattenPositions,
            halt_id: halt_id.to_string(),
            action_id: FLATTEN_ACTION_ID.to_string(),
            policy_sha256: self.config.policy_sha256.clone(),
            account_ids: self.config.account_ids.clone(),
            instrument_ids: self.config.instrument_ids.clone(),
        })?;
        Ok(())
    }

    fn loss_snapshot(&self) -> KillSwitchLossProtectionSnapshot {
        KillSwitchLossProtectionSnapshot {
            daily_bucket: self.daily_bucket,
            daily_realized_pnl: self.daily_realized_pnl,
            cumulative_position_pnl: self
                .cumulative_position_pnl
                .iter()
                .map(|(position_id, value)| {
                    (
                        position_id.clone(),
                        KillSwitchCumulativePositionPnlSnapshot {
                            realized_pnl: value.realized_pnl,
                            last_observed_at_unix_nanos: value.last_observed_at_unix_nanos,
                        },
                    )
                })
                .collect(),
            pending_halt_actions: self.pending_halt_actions.as_ref().map(|pending| {
                KillSwitchPendingHaltActionsSnapshot {
                    next_retry_at_unix_nanos: pending.next_retry_at_unix_nanos,
                    retry_deadline_unix_nanos: pending.retry_deadline_unix_nanos,
                }
            }),
        }
    }

    fn apply_loss_snapshot(&mut self, snapshot: KillSwitchLossProtectionSnapshot) {
        self.daily_bucket = snapshot.daily_bucket;
        self.daily_realized_pnl = snapshot.daily_realized_pnl;
        self.cumulative_position_pnl = snapshot
            .cumulative_position_pnl
            .into_iter()
            .map(|(position_id, value)| {
                (
                    position_id,
                    CumulativePositionPnl {
                        realized_pnl: value.realized_pnl,
                        last_observed_at_unix_nanos: value.last_observed_at_unix_nanos,
                    },
                )
            })
            .collect();
        self.pending_halt_actions =
            snapshot
                .pending_halt_actions
                .map(|pending| PendingHaltActions {
                    state: self.state.clone(),
                    next_retry_at_unix_nanos: pending.next_retry_at_unix_nanos,
                    retry_deadline_unix_nanos: pending.retry_deadline_unix_nanos,
                });
    }

    fn persist_runtime_snapshot_or_fail_closed(&mut self) -> anyhow::Result<()> {
        if let Err(error) = self
            .store
            .write_state_with_loss_snapshot(&self.state, Some(&self.loss_snapshot()))
        {
            let reason = format!("{SNAPSHOT_PERSISTENCE_FAILED_REASON}: {error:?}");
            let failed = KillSwitchState::FailedManualIntervention {
                halt_id: halt_id(&self.state)
                    .unwrap_or(FAIL_CLOSED_RECOVERY_HALT_ID)
                    .to_string(),
                reason: reason.clone(),
            };
            self.pending_halt_actions = None;
            self.admission.replace_kill_switch_state(failed.clone());
            self.state = failed.clone();
            self.persist_failed_state_or_invalidate(&failed)?;
            return Err(anyhow!(reason));
        }
        Ok(())
    }

    fn persist_failed_state_or_invalidate(&self, failed: &KillSwitchState) -> anyhow::Result<()> {
        if let Err(write_error) = self.store.write_state(failed) {
            self.store.invalidate().map_err(|invalidate_error| {
                anyhow!(
                    "kill switch failed-state persistence failed: {write_error:?}; \
                     invalidation failed: {invalidate_error:?}"
                )
            })?;
        }
        Ok(())
    }
}

fn add_millis(unix_nanos: u64, millis: u64) -> u64 {
    unix_nanos.saturating_add(millis.saturating_mul(NANOS_PER_MILLISECOND))
}

pub fn seed_admission_from_kill_switch_store(
    admission: &BoltV3SubmitAdmissionState,
    store: &KillSwitchStore,
) -> anyhow::Result<KillSwitchState> {
    let state = match store
        .load_recovery_state()
        .map_err(|error| anyhow!("kill switch recovery load failed: {error:?}"))?
    {
        KillSwitchRecoveryState::Recovered(state) => state,
        KillSwitchRecoveryState::FailClosed { reason, state } => {
            fail_closed_recovery_state(reason, state)
        }
    };
    admission.replace_kill_switch_state(state.clone());
    Ok(state)
}

fn fail_closed_recovery_state(
    reason: KillSwitchRecoveryReason,
    state: Option<KillSwitchState>,
) -> KillSwitchState {
    match state {
        Some(state @ KillSwitchState::Halting { .. })
        | Some(state @ KillSwitchState::FailedManualIntervention { .. }) => state,
        Some(state) => KillSwitchState::FailedManualIntervention {
            halt_id: halt_id(&state)
                .unwrap_or(FAIL_CLOSED_RECOVERY_HALT_ID)
                .to_string(),
            reason: recovery_reason_label(reason).to_string(),
        },
        None => KillSwitchState::FailedManualIntervention {
            halt_id: FAIL_CLOSED_RECOVERY_HALT_ID.to_string(),
            reason: recovery_reason_label(reason).to_string(),
        },
    }
}

fn halt_id(state: &KillSwitchState) -> Option<&str> {
    match state {
        KillSwitchState::Halting { halt_id, .. }
        | KillSwitchState::Halted { halt_id, .. }
        | KillSwitchState::Cancelling { halt_id }
        | KillSwitchState::Flattening { halt_id }
        | KillSwitchState::Flat { halt_id }
        | KillSwitchState::FailedManualIntervention { halt_id, .. } => Some(halt_id),
        KillSwitchState::Armed => None,
    }
}

fn recovery_reason_label(reason: KillSwitchRecoveryReason) -> &'static str {
    match reason {
        KillSwitchRecoveryReason::MissingEvidence => "missing_evidence",
        KillSwitchRecoveryReason::MissingLossProtectionSnapshot => {
            "missing_loss_protection_snapshot"
        }
        KillSwitchRecoveryReason::CorruptEvidence => "corrupt_evidence",
        KillSwitchRecoveryReason::OversizedEvidence => "oversized_evidence",
        KillSwitchRecoveryReason::UnsupportedSchemaVersion => "unsupported_schema_version",
        KillSwitchRecoveryReason::UnresolvedHalt => "unresolved_halt",
    }
}

fn inert_transition_context() -> KillSwitchTransitionContext {
    KillSwitchTransitionContext {
        state_write_succeeded: false,
        durable_halt_evidence_recorded: false,
        operator_authorized: false,
        manual_reset_evidence_valid: false,
        mandatory_proof_streams_fresh: false,
        no_outstanding_order_risk: false,
        no_open_positions: false,
        no_pending_entry_risk: false,
    }
}

fn position_realized_pnl_observation(
    event: &PositionEvent,
) -> Option<PositionRealizedPnlObservation> {
    match event {
        PositionEvent::PositionOpened(_) => None,
        PositionEvent::PositionChanged(changed) => {
            let realized_pnl = money_decimal(changed.realized_pnl?);
            Some(PositionRealizedPnlObservation {
                account_id: changed.account_id.to_string(),
                instrument_id: changed.instrument_id.to_string(),
                position_id: changed.position_id.to_string(),
                observed: RealizedPnlObservation {
                    source: "nt_position_changed",
                    observed_at_unix_nanos: changed.ts_event.as_u64(),
                    realized_pnl,
                },
                cumulative_realized_pnl: true,
                closes_position: false,
            })
        }
        PositionEvent::PositionClosed(closed) => {
            let realized_pnl = money_decimal(closed.realized_pnl?);
            Some(PositionRealizedPnlObservation {
                account_id: closed.account_id.to_string(),
                instrument_id: closed.instrument_id.to_string(),
                position_id: closed.position_id.to_string(),
                observed: RealizedPnlObservation {
                    source: "nt_position_closed",
                    observed_at_unix_nanos: closed.ts_event.as_u64(),
                    realized_pnl,
                },
                cumulative_realized_pnl: true,
                closes_position: true,
            })
        }
        PositionEvent::PositionAdjusted(adjusted) => {
            let pnl_change = money_decimal(adjusted.pnl_change?);
            Some(PositionRealizedPnlObservation {
                account_id: adjusted.account_id.to_string(),
                instrument_id: adjusted.instrument_id.to_string(),
                position_id: adjusted.position_id.to_string(),
                observed: RealizedPnlObservation {
                    source: "nt_position_adjusted",
                    observed_at_unix_nanos: adjusted.ts_event.as_u64(),
                    realized_pnl: pnl_change,
                },
                cumulative_realized_pnl: false,
                closes_position: false,
            })
        }
    }
}

fn money_decimal(money: Money) -> Decimal {
    money.as_decimal()
}
