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
        KillSwitchRecoveryReason, KillSwitchRecoveryState, KillSwitchStore,
    },
    bolt_v3_submit_admission::{BoltV3KillSwitchForcedReductionPolicy, BoltV3SubmitAdmissionState},
};

const NANOS_PER_UTC_DAY: u64 = 86_400_000_000_000;
const FAIL_CLOSED_RECOVERY_HALT_ID: &str = "kill-switch-recovery-fail-closed";
const LOSS_TRIGGER_REASON: &str = "daily_realized_loss_limit";
const CANCEL_ACTION_ID: &str = "cancel-open-orders";
const FLATTEN_ACTION_ID: &str = "flatten-positions";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillSwitchLossProtectionConfig {
    pub daily_realized_loss_limit: Decimal,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSwitchLossActionKind {
    CancelOpenOrders,
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

pub struct KillSwitchLossProtection {
    config: KillSwitchLossProtectionConfig,
    admission: Arc<BoltV3SubmitAdmissionState>,
    store: KillSwitchStore,
    action_sink: Rc<dyn KillSwitchLossActionSink>,
    state: KillSwitchState,
    daily_bucket: Option<u64>,
    daily_realized_pnl: Decimal,
    cumulative_position_pnl: BTreeMap<String, Decimal>,
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
        })
    }

    pub fn store(&self) -> &KillSwitchStore {
        &self.store
    }

    pub fn seed_from_store(&mut self) -> anyhow::Result<KillSwitchState> {
        let state = seed_admission_from_kill_switch_store(&self.admission, &self.store)?;
        self.state = state.clone();
        Ok(state)
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
        let observed = if observation.cumulative_realized_pnl {
            let previous = self
                .cumulative_position_pnl
                .insert(
                    observation.position_id.clone(),
                    observation.observed.realized_pnl,
                )
                .unwrap_or(Decimal::ZERO);
            RealizedPnlObservation {
                realized_pnl: observation.observed.realized_pnl - previous,
                ..observation.observed
            }
        } else {
            observation.observed
        };
        self.record_realized_pnl(observed)
    }

    pub fn record_realized_pnl(
        &mut self,
        observation: RealizedPnlObservation,
    ) -> anyhow::Result<Option<KillSwitchState>> {
        if !matches!(self.state, KillSwitchState::Armed) {
            return Ok(None);
        }

        let bucket = observation.observed_at_unix_nanos / NANOS_PER_UTC_DAY;
        if self.daily_bucket != Some(bucket) {
            self.daily_bucket = Some(bucket);
            self.daily_realized_pnl = Decimal::ZERO;
            self.cumulative_position_pnl.clear();
        }
        self.daily_realized_pnl += observation.realized_pnl;

        if self.daily_realized_pnl >= Decimal::ZERO
            || -self.daily_realized_pnl < self.config.daily_realized_loss_limit
        {
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

        self.store
            .write_state(&halting)
            .map_err(|error| anyhow!("daily realized loss halt persistence failed: {error:?}"))?;
        self.admission.replace_kill_switch_state(halting.clone());
        self.state = halting.clone();
        self.emit_halt_actions(&halting)?;
        Ok(Some(halting))
    }

    fn emit_halt_actions(&self, state: &KillSwitchState) -> anyhow::Result<()> {
        let Some(halt_id) = halt_id(state) else {
            return Ok(());
        };
        for (kind, action_id) in [
            (KillSwitchLossActionKind::CancelOpenOrders, CANCEL_ACTION_ID),
            (
                KillSwitchLossActionKind::FlattenPositions,
                FLATTEN_ACTION_ID,
            ),
        ] {
            self.action_sink.emit(KillSwitchLossAction {
                kind,
                halt_id: halt_id.to_string(),
                action_id: action_id.to_string(),
                policy_sha256: self.config.policy_sha256.clone(),
                account_ids: self.config.account_ids.clone(),
                instrument_ids: self.config.instrument_ids.clone(),
            })?;
        }
        Ok(())
    }
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
        | KillSwitchState::Flat { halt_id }
        | KillSwitchState::FailedManualIntervention { halt_id, .. } => Some(halt_id),
        KillSwitchState::Armed => None,
    }
}

fn recovery_reason_label(reason: KillSwitchRecoveryReason) -> &'static str {
    match reason {
        KillSwitchRecoveryReason::MissingEvidence => "missing_evidence",
        KillSwitchRecoveryReason::CorruptEvidence => "corrupt_evidence",
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
            })
        }
    }
}

fn money_decimal(money: Money) -> Decimal {
    money.as_decimal()
}
