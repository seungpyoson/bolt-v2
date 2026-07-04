//! Loss-governor manual recovery is a safety check, not an operator override.
//! It only clears a loss-governor halt when the durable kill-switch snapshot
//! proves the configured loss condition has passed; missing durable dimensions
//! fail closed. Operators who need a halt cleared while a configured condition is
//! still breached must use the sanctioned alternative: a reviewed config change
//! to the loss limits, or wait for the condition to clear.
//!
//! Stop the node before running this command. The kill-switch state file remains
//! last-writer-wins, so a live node can rewrite the state after the CLI writes
//! it. Manual-recovery audit attempts are stored in a sibling append-only JSONL
//! file, so state races cannot erase the audit trail.
//!
//! `evidence_path` and `evidence_sha256` are operator-attested audit metadata.
//! This command never opens the evidence file and never hash-verifies it; the
//! values are recorded so reviewers can find and verify the external evidence.

use std::{
    fmt,
    path::{Path, PathBuf},
};

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
        LossAdmissionDecision, LossGovernorPolicy, LossSnapshot, LossSourceObservationTimestamps,
        evaluate_loss_admission,
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
    RecoveryRefused {
        reason: LossGovernorManualRecoveryRefusal,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LossGovernorManualRecoveryRefusal {
    StaleEvidence {
        observed_at_ns: u64,
        now_ns: u64,
    },
    StaleSnapshot {
        observed_at_ns: u64,
        now_ns: u64,
        max_snapshot_age_ns: u64,
    },
    StoredLossBreach {
        check: &'static str,
        stored_loss: Decimal,
        limit: Decimal,
    },
    MissingDimensionFailClosed {
        dimension: &'static str,
        required_by: &'static str,
    },
    RecoveryMode {
        current_state: TradingState,
        recovery_mode: LossGovernorRecoveryMode,
        decision_accepted: bool,
    },
}

impl fmt::Display for LossGovernorManualRecoveryRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleEvidence {
                observed_at_ns,
                now_ns,
            } => write!(
                f,
                "stale evidence check refused: evidence observed_at_ns={observed_at_ns} is after now_ns={now_ns}"
            ),
            Self::StaleSnapshot {
                observed_at_ns,
                now_ns,
                max_snapshot_age_ns,
            } => write!(
                f,
                "stale snapshot check refused: snapshot observed_at_ns={observed_at_ns}, now_ns={now_ns}, max_snapshot_age_ns={max_snapshot_age_ns}"
            ),
            Self::StoredLossBreach {
                check,
                stored_loss,
                limit,
            } => write!(
                f,
                "{check} refused: stored_loss={stored_loss} limit={limit}"
            ),
            Self::MissingDimensionFailClosed {
                dimension,
                required_by,
            } => write!(
                f,
                "missing-dimension fail-closed: {required_by} requires {dimension}, but the kill-switch store has no durable value for that dimension"
            ),
            Self::RecoveryMode {
                current_state,
                recovery_mode,
                decision_accepted,
            } => write!(
                f,
                "recovery-mode check refused: recovery_mode={recovery_mode:?} current_state={current_state:?} decision_accepted={decision_accepted}"
            ),
        }
    }
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
            Self::RecoveryRefused { reason } => write!(
                f,
                "loss-governor manual recovery refused by the recovery state machine: {reason}"
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
        });
    if target != Some(TradingState::Active) {
        return Err(LossGovernorManualRecoveryError::RecoveryRefused {
            reason: manual_recovery_refusal(
                &loss_policy,
                &action_policy,
                current_trading_state,
                &decision,
                &snapshot,
                &evidence,
                command.now_ns,
            ),
        });
    }

    let previous_state = current_state.kind();
    let manual_recovery = KillSwitchLossGovernorManualRecoveryRecord {
        operator_id: evidence.operator_id().to_string(),
        evidence_path: evidence.evidence_path().to_string(),
        evidence_sha256: evidence.evidence_sha256().to_string(),
        observed_at_ns: evidence.observed_at_ns(),
        recorded_at_ns: command.now_ns,
    };
    let manual_recovery_count =
        persist_manual_recovery_attempt(&store, &current_state, loss_protection, manual_recovery)?;

    Ok(LossGovernorManualRecoveryOutcome {
        state_path: store.path().to_path_buf(),
        previous_state,
        recovered_state: KillSwitchStateKind::Armed,
        manual_recovery_count,
    })
}

trait LossGovernorManualRecoveryStoreWriter {
    fn path(&self) -> &Path;

    fn append_loss_governor_manual_recovery(
        &self,
        manual_recovery: KillSwitchLossGovernorManualRecoveryRecord,
    ) -> Result<usize, KillSwitchStoreError>;

    fn write_state_with_loss_snapshot(
        &self,
        state: &KillSwitchState,
        loss_protection: Option<&KillSwitchLossProtectionSnapshot>,
    ) -> Result<(), KillSwitchStoreError>;
}

impl LossGovernorManualRecoveryStoreWriter for KillSwitchStore {
    fn path(&self) -> &Path {
        KillSwitchStore::path(self)
    }

    fn append_loss_governor_manual_recovery(
        &self,
        manual_recovery: KillSwitchLossGovernorManualRecoveryRecord,
    ) -> Result<usize, KillSwitchStoreError> {
        KillSwitchStore::append_loss_governor_manual_recovery(self, manual_recovery)
    }

    fn write_state_with_loss_snapshot(
        &self,
        state: &KillSwitchState,
        loss_protection: Option<&KillSwitchLossProtectionSnapshot>,
    ) -> Result<(), KillSwitchStoreError> {
        KillSwitchStore::write_state_with_loss_snapshot(self, state, loss_protection)
    }
}

fn persist_manual_recovery_attempt(
    store: &impl LossGovernorManualRecoveryStoreWriter,
    current_state: &KillSwitchState,
    loss_protection: &KillSwitchLossProtectionSnapshot,
    manual_recovery: KillSwitchLossGovernorManualRecoveryRecord,
) -> Result<usize, LossGovernorManualRecoveryError> {
    let manual_recovery_count = match store.append_loss_governor_manual_recovery(manual_recovery) {
        Ok(count) => count,
        Err(recovery_error) => {
            let failed = failed_manual_intervention_state(
                current_state,
                format!("loss governor manual recovery audit write failed: {recovery_error:?}"),
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
    if let Err(recovery_error) =
        store.write_state_with_loss_snapshot(&KillSwitchState::Armed, Some(loss_protection))
    {
        let failed = failed_manual_intervention_state(
            current_state,
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
    Ok(manual_recovery_count)
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
        per_trade_pnl: None,
        daily_pnl: Some(snapshot.daily_realized_pnl),
        rolling_pnl: None,
        current_equity: None,
        peak_equity: None,
        source_observations: LossSourceObservationTimestamps::unobserved(),
    }
}

fn manual_recovery_refusal(
    loss_policy: &LossGovernorPolicy,
    action_policy: &LossGovernorHaltActionPolicy,
    current_state: TradingState,
    decision: &LossAdmissionDecision,
    snapshot: &LossSnapshot,
    evidence: &LossGovernorManualRecoveryEvidence,
    now_ns: u64,
) -> LossGovernorManualRecoveryRefusal {
    if evidence.observed_at_ns() > now_ns {
        return LossGovernorManualRecoveryRefusal::StaleEvidence {
            observed_at_ns: evidence.observed_at_ns(),
            now_ns,
        };
    }
    if snapshot.source.trim().is_empty()
        || snapshot.observed_at_ns > now_ns
        || now_ns - snapshot.observed_at_ns > loss_policy.max_snapshot_age_ns
    {
        return LossGovernorManualRecoveryRefusal::StaleSnapshot {
            observed_at_ns: snapshot.observed_at_ns,
            now_ns,
            max_snapshot_age_ns: loss_policy.max_snapshot_age_ns,
        };
    }
    if let Some(refusal) = stored_loss_breach_refusal(loss_policy, snapshot) {
        return refusal;
    }
    if let Some(refusal) = missing_dimension_refusal(loss_policy, snapshot) {
        return refusal;
    }
    LossGovernorManualRecoveryRefusal::RecoveryMode {
        current_state,
        recovery_mode: action_policy.recovery_mode,
        decision_accepted: decision.accepted,
    }
}

fn stored_loss_breach_refusal(
    policy: &LossGovernorPolicy,
    snapshot: &LossSnapshot,
) -> Option<LossGovernorManualRecoveryRefusal> {
    if let (Some(limit), Some(stored_loss)) = (policy.max_daily_loss, snapshot.daily_pnl) {
        if loss_breaches(stored_loss, limit) {
            return Some(LossGovernorManualRecoveryRefusal::StoredLossBreach {
                check: "daily_loss_limit",
                stored_loss,
                limit,
            });
        }
    }
    if let (Some(limit), Some(stored_loss)) = (policy.max_rolling_loss, snapshot.rolling_pnl) {
        if loss_breaches(stored_loss, limit) {
            return Some(LossGovernorManualRecoveryRefusal::StoredLossBreach {
                check: "rolling_loss_limit",
                stored_loss,
                limit,
            });
        }
    }
    None
}

fn missing_dimension_refusal(
    policy: &LossGovernorPolicy,
    snapshot: &LossSnapshot,
) -> Option<LossGovernorManualRecoveryRefusal> {
    if policy.max_per_trade_loss.is_some() && snapshot.per_trade_pnl.is_none() {
        return Some(
            LossGovernorManualRecoveryRefusal::MissingDimensionFailClosed {
                dimension: "per_trade_pnl",
                required_by: "risk.loss_governor.max_per_trade_loss",
            },
        );
    }
    if policy.max_daily_loss.is_some() && snapshot.daily_pnl.is_none() {
        return Some(
            LossGovernorManualRecoveryRefusal::MissingDimensionFailClosed {
                dimension: "daily_pnl",
                required_by: "risk.loss_governor.max_daily_loss",
            },
        );
    }
    if policy.max_rolling_loss.is_some() && snapshot.rolling_pnl.is_none() {
        return Some(
            LossGovernorManualRecoveryRefusal::MissingDimensionFailClosed {
                dimension: "rolling_pnl",
                required_by: "risk.loss_governor.max_rolling_loss",
            },
        );
    }
    if policy.max_drawdown.is_some() && snapshot.current_equity.is_none() {
        return Some(
            LossGovernorManualRecoveryRefusal::MissingDimensionFailClosed {
                dimension: "current_equity",
                required_by: "risk.loss_governor.max_drawdown",
            },
        );
    }
    if policy.max_drawdown.is_some() && snapshot.peak_equity.is_none() {
        return Some(
            LossGovernorManualRecoveryRefusal::MissingDimensionFailClosed {
                dimension: "peak_equity",
                required_by: "risk.loss_governor.max_drawdown",
            },
        );
    }
    None
}

fn loss_breaches(pnl: Decimal, limit: Decimal) -> bool {
    pnl < Decimal::ZERO && -pnl >= limit
}

fn loss_governor_policy(
    block: &LossGovernorBlock,
) -> Result<LossGovernorPolicy, LossGovernorManualRecoveryError> {
    Ok(LossGovernorPolicy {
        max_snapshot_age_ns: block.max_snapshot_age_ns,
        max_per_trade_loss: optional_loss_governor_decimal(
            "risk.loss_governor.max_per_trade_loss",
            block.max_per_trade_loss.as_deref(),
        )?,
        max_daily_loss: optional_loss_governor_decimal(
            "risk.loss_governor.max_daily_loss",
            block.max_daily_loss.as_deref(),
        )?,
        max_rolling_loss: optional_loss_governor_decimal(
            "risk.loss_governor.max_rolling_loss",
            block.max_rolling_loss.as_deref(),
        )?,
        max_drawdown: optional_loss_governor_decimal(
            "risk.loss_governor.max_drawdown",
            block.max_drawdown.as_deref(),
        )?,
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

fn optional_loss_governor_decimal(
    label: &'static str,
    value: Option<&str>,
) -> Result<Option<Decimal>, LossGovernorManualRecoveryError> {
    value
        .map(|value| {
            parse_decimal_string(value).map_err(|reason| {
                LossGovernorManualRecoveryError::InvalidLossGovernorDecimal { label, reason }
            })
        })
        .transpose()
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

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::{BTreeMap, VecDeque},
        io,
    };

    use super::*;
    use crate::bolt_v3_kill_switch::KillSwitchHaltTrigger;

    struct FakeManualRecoveryStore {
        path: PathBuf,
        append_calls: RefCell<usize>,
        write_results: RefCell<VecDeque<bool>>,
        written_states: RefCell<Vec<KillSwitchState>>,
    }

    impl FakeManualRecoveryStore {
        fn new(write_results: impl IntoIterator<Item = bool>) -> Self {
            Self {
                path: PathBuf::from("state/kill-switch.json"),
                append_calls: RefCell::new(0),
                write_results: RefCell::new(write_results.into_iter().collect()),
                written_states: RefCell::new(Vec::new()),
            }
        }

        fn written_states(&self) -> Vec<KillSwitchState> {
            self.written_states.borrow().clone()
        }
    }

    impl LossGovernorManualRecoveryStoreWriter for FakeManualRecoveryStore {
        fn path(&self) -> &Path {
            &self.path
        }

        fn append_loss_governor_manual_recovery(
            &self,
            _manual_recovery: KillSwitchLossGovernorManualRecoveryRecord,
        ) -> Result<usize, KillSwitchStoreError> {
            *self.append_calls.borrow_mut() += 1;
            Ok(1)
        }

        fn write_state_with_loss_snapshot(
            &self,
            state: &KillSwitchState,
            _loss_protection: Option<&KillSwitchLossProtectionSnapshot>,
        ) -> Result<(), KillSwitchStoreError> {
            self.written_states.borrow_mut().push(state.clone());
            match self.write_results.borrow_mut().pop_front() {
                Some(true) => Ok(()),
                Some(false) | None => Err(synthetic_store_error(&self.path)),
            }
        }
    }

    fn synthetic_store_error(path: &Path) -> KillSwitchStoreError {
        KillSwitchStoreError::Io {
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::Other, "synthetic write failure"),
        }
    }

    fn recovery_state() -> KillSwitchState {
        KillSwitchState::Halted {
            halt_id: "halt-loss-governor-1".to_string(),
            trigger: KillSwitchHaltTrigger::loss_governor_breach(
                "loss-governor",
                2_000,
                "daily loss cap breached",
            ),
        }
    }

    fn loss_snapshot() -> KillSwitchLossProtectionSnapshot {
        KillSwitchLossProtectionSnapshot {
            daily_bucket: Some(19_875),
            daily_realized_pnl: Decimal::ZERO,
            settlement_currency: Some("USDC".to_string()),
            cumulative_position_pnl: BTreeMap::new(),
            closed_position_pnl: BTreeMap::new(),
            adjusted_position_pnl: BTreeMap::new(),
            pending_halt_actions: None,
        }
    }

    fn manual_recovery_record() -> KillSwitchLossGovernorManualRecoveryRecord {
        KillSwitchLossGovernorManualRecoveryRecord {
            operator_id: "operator-primary".to_string(),
            evidence_path: "loss-governor/manual-recovery.json".to_string(),
            evidence_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            observed_at_ns: 2_500,
            recorded_at_ns: 2_600,
        }
    }

    #[test]
    fn failed_recovery_state_write_persists_failed_manual_intervention() {
        let store = FakeManualRecoveryStore::new([false, true]);

        let error = persist_manual_recovery_attempt(
            &store,
            &recovery_state(),
            &loss_snapshot(),
            manual_recovery_record(),
        )
        .expect_err("state write failure should surface after failed state is persisted");

        assert!(matches!(
            error,
            LossGovernorManualRecoveryError::StoreWriteFailed { .. }
        ));
        assert_eq!(*store.append_calls.borrow(), 1);
        let written_states = store.written_states();
        assert_eq!(written_states.len(), 2);
        assert_eq!(written_states[0], KillSwitchState::Armed);
        assert!(matches!(
            written_states[1],
            KillSwitchState::FailedManualIntervention { .. }
        ));
    }

    #[test]
    fn failed_recovery_state_write_reports_when_failed_state_also_fails() {
        let store = FakeManualRecoveryStore::new([false, false]);

        let error = persist_manual_recovery_attempt(
            &store,
            &recovery_state(),
            &loss_snapshot(),
            manual_recovery_record(),
        )
        .expect_err("both failed writes should surface together");

        assert!(matches!(
            error,
            LossGovernorManualRecoveryError::FailedStateWriteFailed { .. }
        ));
        assert_eq!(*store.append_calls.borrow(), 1);
        let written_states = store.written_states();
        assert_eq!(written_states.len(), 2);
        assert_eq!(written_states[0], KillSwitchState::Armed);
        assert!(matches!(
            written_states[1],
            KillSwitchState::FailedManualIntervention { .. }
        ));
    }
}
