use std::{fmt, path::PathBuf};

use nautilus_model::enums::TradingState;
use rust_decimal::Decimal;

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LossGovernorBlock},
    bolt_v3_kill_switch::{KillSwitchHaltTriggerKind, KillSwitchState, KillSwitchStateKind},
    bolt_v3_kill_switch_store::{
        KillSwitchLossGovernorManualRecoveryRecord, KillSwitchLossProtectionSnapshot,
        KillSwitchRecoveryReason, KillSwitchRecoveryRecord, KillSwitchRecoveryState,
        KillSwitchStore, KillSwitchStoreError,
    },
    bolt_v3_loss_governor::{
        LossGovernorPolicy, LossSnapshot, LossSourceObservationTimestamps, evaluate_loss_admission,
    },
    bolt_v3_loss_halt_actions::{
        LossGovernorHaltActionPolicy, LossGovernorManualRecoveryEvidence,
        LossGovernorManualRecoveryEvidenceError, LossGovernorManualRecoveryRequest,
        LossGovernorRecoveryMode, LossGovernorTradingStateAction,
        next_loss_governor_manual_recovery_trading_state,
    },
    bolt_v3_validate::parse_decimal_string,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossGovernorManualRecoveryCommand {
    pub operator_id: String,
    pub evidence_path: String,
    pub evidence_sha256: String,
    pub observed_at_ns: u64,
    pub now_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossGovernorManualRecoveryOutcome {
    pub state_path: PathBuf,
    pub previous_state: KillSwitchStateKind,
    pub recovered_state: KillSwitchStateKind,
    pub manual_recovery_count: usize,
}

#[derive(Debug)]
pub enum LossGovernorManualRecoveryError {
    MissingKillSwitchConfig,
    KillSwitchDisabled,
    MissingLossGovernorConfig,
    LossGovernorDisabled,
    MissingLossGovernorField {
        label: &'static str,
    },
    InvalidLossGovernorDecimal {
        label: &'static str,
        reason: String,
    },
    InvalidManualRecoveryEvidence(LossGovernorManualRecoveryEvidenceError),
    UnauthorizedOperator {
        operator_id: String,
    },
    StoreLoad(KillSwitchStoreError),
    MissingStore {
        path: PathBuf,
    },
    StoreFailClosed {
        path: PathBuf,
        reason: KillSwitchRecoveryReason,
    },
    MissingLossProtectionSnapshot {
        path: PathBuf,
    },
    UnsupportedState {
        state: KillSwitchStateKind,
    },
    NonLossGovernorState {
        state: KillSwitchStateKind,
    },
    RecoveryRefused,
    StoreWriteFailed {
        path: PathBuf,
        source: KillSwitchStoreError,
    },
    FailedStateWriteFailed {
        path: PathBuf,
        recovery_error: KillSwitchStoreError,
        failed_error: KillSwitchStoreError,
    },
}

impl fmt::Display for LossGovernorManualRecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingKillSwitchConfig => {
                write!(f, "risk.kill_switch block is required for manual recovery")
            }
            Self::KillSwitchDisabled => {
                write!(
                    f,
                    "risk.kill_switch.enabled=true is required for manual recovery"
                )
            }
            Self::MissingLossGovernorConfig => {
                write!(
                    f,
                    "risk.loss_governor block is required for manual recovery"
                )
            }
            Self::LossGovernorDisabled => {
                write!(
                    f,
                    "risk.loss_governor.enabled=true is required for manual recovery"
                )
            }
            Self::MissingLossGovernorField { label } => {
                write!(f, "{label} is required for manual recovery")
            }
            Self::InvalidLossGovernorDecimal { label, reason } => {
                write!(f, "{label} is not a valid decimal string: {reason}")
            }
            Self::InvalidManualRecoveryEvidence(error) => {
                write!(f, "invalid manual recovery evidence: {error:?}")
            }
            Self::UnauthorizedOperator { operator_id } => {
                write!(
                    f,
                    "operator `{operator_id}` is not authorized by risk.kill_switch.authorized_operator_ids"
                )
            }
            Self::StoreLoad(error) => write!(f, "kill-switch store load failed: {error}"),
            Self::MissingStore { path } => write!(
                f,
                "kill-switch state file is missing at {}; refusing to bootstrap during manual recovery",
                path.display()
            ),
            Self::StoreFailClosed { path, reason } => write!(
                f,
                "kill-switch store at {} is fail-closed ({reason}); refusing manual recovery",
                path.display()
            ),
            Self::MissingLossProtectionSnapshot { path } => write!(
                f,
                "kill-switch store at {} has no loss-protection snapshot; refusing manual recovery",
                path.display()
            ),
            Self::UnsupportedState { state } => write!(
                f,
                "loss-governor manual recovery cannot recover kill-switch state {state:?}"
            ),
            Self::NonLossGovernorState { state } => write!(
                f,
                "refusing to recover non-loss-governor kill-switch state {state:?}"
            ),
            Self::RecoveryRefused => write!(
                f,
                "loss-governor manual recovery refused by the recovery state machine"
            ),
            Self::StoreWriteFailed { path, source } => write!(
                f,
                "loss-governor manual recovery write failed for {}; FailedManualIntervention was persisted: {source}",
                path.display()
            ),
            Self::FailedStateWriteFailed {
                path,
                recovery_error,
                failed_error,
            } => write!(
                f,
                "loss-governor manual recovery write failed for {}, and FailedManualIntervention persistence also failed: recovery_error={recovery_error}; failed_state_error={failed_error}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for LossGovernorManualRecoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StoreLoad(error)
            | Self::StoreWriteFailed { source: error, .. }
            | Self::FailedStateWriteFailed {
                recovery_error: error,
                ..
            } => Some(error),
            _ => None,
        }
    }
}

pub fn recover_loss_governor_manual_halt(
    loaded: &LoadedBoltV3Config,
    command: LossGovernorManualRecoveryCommand,
) -> Result<LossGovernorManualRecoveryOutcome, LossGovernorManualRecoveryError> {
    let kill_switch = loaded
        .root
        .risk
        .kill_switch
        .as_ref()
        .ok_or(LossGovernorManualRecoveryError::MissingKillSwitchConfig)?;
    if !kill_switch.enabled {
        return Err(LossGovernorManualRecoveryError::KillSwitchDisabled);
    }

    let loss_governor = loaded
        .root
        .risk
        .loss_governor
        .as_ref()
        .ok_or(LossGovernorManualRecoveryError::MissingLossGovernorConfig)?;
    if !loss_governor.enabled {
        return Err(LossGovernorManualRecoveryError::LossGovernorDisabled);
    }

    let action_policy = halt_action_policy(loss_governor)?;
    let evidence = LossGovernorManualRecoveryEvidence::new(
        command.operator_id,
        command.evidence_path,
        command.evidence_sha256,
        command.observed_at_ns,
        action_policy.manual_recovery_evidence_max_path_bytes,
    )
    .map_err(LossGovernorManualRecoveryError::InvalidManualRecoveryEvidence)?;
    if !kill_switch
        .authorized_operator_ids
        .iter()
        .any(|operator_id| operator_id == evidence.operator_id())
    {
        return Err(LossGovernorManualRecoveryError::UnauthorizedOperator {
            operator_id: evidence.operator_id().to_string(),
        });
    }

    let loss_policy = loss_governor_policy(loss_governor)?;
    let store = KillSwitchStore::from_root_config_path(&loaded.root_path, kill_switch);
    let record = store
        .load_recovery_record()
        .map_err(LossGovernorManualRecoveryError::StoreLoad)?;
    let current_state = recoverable_current_state(&record, store.path().to_path_buf())?;
    let current_trading_state = loss_governor_recoverable_trading_state(&current_state)?;
    let loss_protection = record.loss_protection.as_ref().ok_or_else(|| {
        LossGovernorManualRecoveryError::MissingLossProtectionSnapshot {
            path: store.path().to_path_buf(),
        }
    })?;

    let snapshot = manual_recovery_loss_snapshot(loss_protection, &evidence);
    let decision = evaluate_loss_admission(&loss_policy, Some(&snapshot), command.now_ns);
    let target =
        next_loss_governor_manual_recovery_trading_state(LossGovernorManualRecoveryRequest {
            policy: &action_policy,
            current_state: current_trading_state,
            decision: &decision,
            snapshot: Some(&snapshot),
            now_ns: command.now_ns,
            max_snapshot_age_ns: loss_policy.max_snapshot_age_ns,
            evidence: Some(&evidence),
            max_evidence_path_bytes: action_policy.manual_recovery_evidence_max_path_bytes,
        })
        .ok_or(LossGovernorManualRecoveryError::RecoveryRefused)?;
    if target != TradingState::Active {
        return Err(LossGovernorManualRecoveryError::RecoveryRefused);
    }

    let previous_state = current_state.kind();
    let manual_recovery = KillSwitchLossGovernorManualRecoveryRecord {
        operator_id: evidence.operator_id().to_string(),
        evidence_path: evidence.evidence_path().to_string(),
        evidence_sha256: evidence.evidence_sha256().to_string(),
        observed_at_ns: evidence.observed_at_ns(),
        recorded_at_ns: command.now_ns,
    };
    let manual_recovery_count = match store.write_loss_governor_manual_recovery(
        &record,
        &KillSwitchState::Armed,
        loss_protection,
        manual_recovery,
    ) {
        Ok(count) => count,
        Err(recovery_error) => {
            let failed = failed_manual_intervention_state(
                &current_state,
                format!("loss governor manual recovery write failed: {recovery_error:?}"),
            );
            return match store.write_state_with_loss_snapshot(&failed, Some(loss_protection)) {
                Ok(()) => Err(LossGovernorManualRecoveryError::StoreWriteFailed {
                    path: store.path().to_path_buf(),
                    source: recovery_error,
                }),
                Err(failed_error) => Err(LossGovernorManualRecoveryError::FailedStateWriteFailed {
                    path: store.path().to_path_buf(),
                    recovery_error,
                    failed_error,
                }),
            };
        }
    };

    Ok(LossGovernorManualRecoveryOutcome {
        state_path: store.path().to_path_buf(),
        previous_state,
        recovered_state: KillSwitchStateKind::Armed,
        manual_recovery_count,
    })
}

fn recoverable_current_state(
    record: &KillSwitchRecoveryRecord,
    path: PathBuf,
) -> Result<KillSwitchState, LossGovernorManualRecoveryError> {
    match &record.recovery_state {
        KillSwitchRecoveryState::Recovered(state) => Ok(state.clone()),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::MissingEvidence,
            state: None,
        } => Err(LossGovernorManualRecoveryError::MissingStore { path }),
        KillSwitchRecoveryState::FailClosed {
            reason: KillSwitchRecoveryReason::UnresolvedHalt,
            state: Some(state),
        } => Ok(state.clone()),
        KillSwitchRecoveryState::FailClosed { reason, .. } => {
            Err(LossGovernorManualRecoveryError::StoreFailClosed {
                path,
                reason: *reason,
            })
        }
    }
}

fn loss_governor_recoverable_trading_state(
    state: &KillSwitchState,
) -> Result<TradingState, LossGovernorManualRecoveryError> {
    match state {
        KillSwitchState::Halting { trigger, .. } | KillSwitchState::Halted { trigger, .. } => {
            if trigger.kind == KillSwitchHaltTriggerKind::LossGovernorBreach {
                Ok(TradingState::Reducing)
            } else {
                Err(LossGovernorManualRecoveryError::NonLossGovernorState {
                    state: state.kind(),
                })
            }
        }
        _ => Err(LossGovernorManualRecoveryError::UnsupportedState {
            state: state.kind(),
        }),
    }
}

fn failed_manual_intervention_state(state: &KillSwitchState, reason: String) -> KillSwitchState {
    let halt_id = match state {
        KillSwitchState::Halting { halt_id, .. } | KillSwitchState::Halted { halt_id, .. } => {
            halt_id.clone()
        }
        _ => unreachable!("manual recovery write failure requires a recoverable halt"),
    };
    KillSwitchState::FailedManualIntervention { halt_id, reason }
}

fn manual_recovery_loss_snapshot(
    snapshot: &KillSwitchLossProtectionSnapshot,
    evidence: &LossGovernorManualRecoveryEvidence,
) -> LossSnapshot {
    LossSnapshot {
        source: evidence.evidence_path().to_string(),
        observed_at_ns: evidence.observed_at_ns(),
        per_trade_pnl: Some(Decimal::ZERO),
        daily_pnl: Some(snapshot.daily_realized_pnl),
        rolling_pnl: Some(snapshot.daily_realized_pnl),
        current_equity: Some(Decimal::ZERO),
        peak_equity: Some(Decimal::ZERO),
        source_observations: LossSourceObservationTimestamps::unobserved(),
    }
}

fn loss_governor_policy(
    block: &LossGovernorBlock,
) -> Result<LossGovernorPolicy, LossGovernorManualRecoveryError> {
    Ok(LossGovernorPolicy {
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
    })
}

fn halt_action_policy(
    block: &LossGovernorBlock,
) -> Result<LossGovernorHaltActionPolicy, LossGovernorManualRecoveryError> {
    Ok(LossGovernorHaltActionPolicy {
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
    })
}

fn required_loss_governor_decimal(
    label: &'static str,
    value: Option<&str>,
) -> Result<Decimal, LossGovernorManualRecoveryError> {
    let value = value.ok_or(LossGovernorManualRecoveryError::MissingLossGovernorField { label })?;
    parse_decimal_string(value).map_err(|reason| {
        LossGovernorManualRecoveryError::InvalidLossGovernorDecimal { label, reason }
    })
}

fn required_loss_governor_trading_state_action(
    label: &'static str,
    value: Option<LossGovernorTradingStateAction>,
) -> Result<LossGovernorTradingStateAction, LossGovernorManualRecoveryError> {
    value.ok_or(LossGovernorManualRecoveryError::MissingLossGovernorField { label })
}

fn required_loss_governor_recovery_mode(
    label: &'static str,
    value: Option<LossGovernorRecoveryMode>,
) -> Result<LossGovernorRecoveryMode, LossGovernorManualRecoveryError> {
    value.ok_or(LossGovernorManualRecoveryError::MissingLossGovernorField { label })
}

fn required_loss_governor_usize(
    label: &'static str,
    value: Option<usize>,
) -> Result<usize, LossGovernorManualRecoveryError> {
    let value = value.ok_or(LossGovernorManualRecoveryError::MissingLossGovernorField { label })?;
    if value == 0 {
        return Err(LossGovernorManualRecoveryError::MissingLossGovernorField { label });
    }
    Ok(value)
}
