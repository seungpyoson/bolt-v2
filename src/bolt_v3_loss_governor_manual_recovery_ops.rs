//! Loss-governor manual recovery is a safety check, not an operator override.
//! It clears a halt only when the triggering condition has verifiably passed by
//! clock: a daily-loss halt clears only after the UTC day rolls, and a
//! rolling-loss halt clears only when more than the full rolling window has
//! elapsed. Exact equality refuses. The rolling-window authority is the current
//! config value, so a reviewed config change is the sanctioned lever; every
//! configured limit is still re-checked live by the loss governor at the next
//! node start. Per-trade, drawdown, stale-snapshot, and legacy stores without a
//! typed trigger reason require runtime-path recovery.
//!
//! Stop the node before running this command. The kill-switch state file remains
//! last-writer-wins, so a live node can rewrite the state after the CLI writes
//! it. Manual-recovery audit attempts are stored in a sibling unbounded
//! append-only JSONL file, so state races cannot erase the audit trail.
//! Operators rotate that audit file externally. The last audit record for a
//! given attempt is authoritative: `attempted` is written before the state write,
//! followed by `recovered` after a successful state write or `write-failed` when
//! the write fails. An audit trail ending in `attempted` while the state file is
//! `Armed` means recovery succeeded but the terminal audit append was
//! interrupted; the state file is authoritative, so cross-check it.
//! `FailedManualIntervention` is terminal for this command, routes to
//! `UnsupportedState`, and needs out-of-band repair.
//!
//! Interleaving completeness matrix:
//!
//! | Durable operation | Crash before operation | Crash after operation | Torn audit at operation | Non-torn IO error at operation |
//! | --- | --- | --- | --- | --- |
//! | `attempted` append | Store remains the original halt and the audit has no new line; the next invocation runs a normal recovery. Test: `manual_recovery_after_crash_before_attempted_append_recovers_from_original_halt`. | Store remains the original halt and the audit ends in `attempted`; the next invocation retries and can recover, appending a new `attempted`/`recovered` pair. Test: `manual_recovery_after_crash_after_attempted_append_retries_and_recovers`. | Store remains the original halt and the audit has a torn final line; the next invocation returns `RepairManualRecoveryAudit` until the audit is repaired. Test: `manual_recovery_torn_audit_requires_repair_without_touching_halt`. | Store is downgraded to `FailedManualIntervention` and no attempted line is durable; the next invocation treats that state as terminal. Tests: `attempted_audit_non_torn_error_persists_failed_manual_intervention`, `manual_recovery_after_failed_manual_intervention_is_terminal_and_audits`. |
//! | `Armed` state write | Store remains the original halt and the audit ends in `attempted`; the next invocation retries and can recover. Test: `manual_recovery_after_crash_after_attempted_append_retries_and_recovers`. | Store is `Armed` and the audit ends in `attempted`; the next invocation treats `Armed` as authoritative and records an `UnsupportedState` refusal when the audit is appendable. Test: `manual_recovery_after_crash_after_armed_write_treats_armed_state_as_authoritative`. | The state write does not inspect the audit file; a pre-existing torn audit is caught by the preceding `attempted` append, leaving the halt untouched and the next invocation returning `RepairManualRecoveryAudit`. Test: `manual_recovery_torn_audit_requires_repair_without_touching_halt`. | Store remains the original halt until fallback runs; with a successful write-failed audit and failed-state write, it becomes `FailedManualIntervention` and the next invocation is terminal. Tests: `failed_recovery_state_write_persists_failed_manual_intervention`, `manual_recovery_after_failed_manual_intervention_is_terminal_and_audits`. |
//! | `write-failed` append | Store remains the original halt and the audit ends in `attempted`; the next invocation retries and can recover. Test: `manual_recovery_after_crash_after_attempted_append_retries_and_recovers`. | Store remains the original halt and the audit ends in `write-failed`; the next invocation retries from the original halt and can recover. Test: `manual_recovery_after_crash_after_write_failed_append_retries_from_original_halt`. | Store remains the original halt and the audit must be repaired before any failed-state write; the next invocation returns `RepairManualRecoveryAudit`. Tests: `failed_recovery_state_write_with_torn_write_failed_audit_requires_repair_only`, `manual_recovery_torn_audit_requires_repair_without_touching_halt`. | Store is downgraded to `FailedManualIntervention` after logging the audit error; the next invocation treats that state as terminal. Tests: `write_failed_audit_io_error_persists_failed_manual_intervention`, `manual_recovery_after_failed_manual_intervention_is_terminal_and_audits`. |
//! | `FailedManualIntervention` state write | Store remains the original halt and the audit ends in `write-failed`; the next invocation retries from the original halt. Test: `manual_recovery_after_crash_after_write_failed_append_retries_from_original_halt`. | Store is `FailedManualIntervention` and the audit ends in `write-failed`; the next invocation routes to `UnsupportedState` and records a refusal when the audit is appendable. Test: `manual_recovery_after_failed_manual_intervention_is_terminal_and_audits`. | The failed-state write does not inspect the audit file; a torn write-failed audit is detected before this write and leaves the halt untouched, so the next invocation returns `RepairManualRecoveryAudit`. Test: `failed_recovery_state_write_with_torn_write_failed_audit_requires_repair_only`. | Store remains the original halt while the audit ends in `write-failed`; the command returns `FailedStateWriteFailed`, and the next invocation retries from the original halt. Tests: `failed_recovery_state_write_reports_when_failed_state_also_fails`, `manual_recovery_after_crash_after_write_failed_append_retries_from_original_halt`. |
//! | `recovered` append | Store is `Armed` and the audit ends in `attempted`; the next invocation treats `Armed` as authoritative and records an `UnsupportedState` refusal when the audit is appendable. Test: `manual_recovery_after_crash_after_armed_write_treats_armed_state_as_authoritative`. | Store is `Armed` and the audit ends in `recovered`; the next invocation treats `Armed` as authoritative and records an `UnsupportedState` refusal when the audit is appendable. Test: `manual_recovery_after_crash_after_recovered_append_refuses_as_armed_and_audits`. | Store is already `Armed`; the command reports success, the audit tail must be repaired before the next refusal can be audited, and the next invocation returns `RepairManualRecoveryAudit`. Tests: `recovered_audit_torn_error_reports_success_after_armed_state_wins`, `manual_recovery_torn_audit_blocks_armed_refusal_until_repaired`. | Store is already `Armed`; the command reports success after logging, the audit may end at `attempted`, and the next invocation treats `Armed` as authoritative when the audit is appendable. Tests: `recovered_audit_non_torn_error_reports_success_after_armed_state_wins`, `manual_recovery_after_crash_after_armed_write_treats_armed_state_as_authoritative`. |
//!
//! Refusals after config and store-path resolution are audited when the audit
//! path is appendable. Pre-config failures and an unwritable audit path are the
//! non-auditable boundary because no durable audit line can be written there.
//!
//! `evidence_path` and `evidence_sha256` are operator-attested audit metadata.
//! This command never opens the evidence file and never hash-verifies it; the
//! values are recorded so reviewers can find and verify the external evidence.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use nautilus_model::enums::TradingState;

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LossGovernorBlock},
    bolt_v3_kill_switch::{KillSwitchHaltTriggerKind, KillSwitchState, KillSwitchStateKind},
    bolt_v3_kill_switch_store::{
        KillSwitchLossGovernorManualRecoveryOutcome, KillSwitchLossGovernorManualRecoveryRecord,
        KillSwitchLossProtectionSnapshot, KillSwitchRecoveryReason, KillSwitchRecoveryRecord,
        KillSwitchRecoveryState, KillSwitchStore, KillSwitchStoreError,
    },
    bolt_v3_loss_governor::LossHaltReason,
    bolt_v3_loss_halt_actions::{
        LossGovernorClockManualRecoveryRefusal, LossGovernorClockManualRecoveryRequest,
        LossGovernorHaltActionPolicy, LossGovernorManualRecoveryEvidence,
        LossGovernorManualRecoveryEvidenceError, LossGovernorRecoveryMode,
        LossGovernorTradingStateAction,
        next_loss_governor_clock_verified_manual_recovery_trading_state,
    },
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
    RepairManualRecoveryAudit {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LossGovernorManualRecoveryRefusal {
    TriggerClock(LossGovernorClockManualRecoveryRefusal),
}

impl fmt::Display for LossGovernorManualRecoveryRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TriggerClock(reason) => write_clock_refusal(f, *reason),
        }
    }
}

fn write_clock_refusal(
    f: &mut fmt::Formatter<'_>,
    reason: LossGovernorClockManualRecoveryRefusal,
) -> fmt::Result {
    match reason {
        LossGovernorClockManualRecoveryRefusal::IneligibleTradingState { current_state } => {
            write!(
                f,
                "recovery-state check refused: current_state={current_state:?} is not a loss-governor halt state"
            )
        }
        LossGovernorClockManualRecoveryRefusal::LegacyStoreMissingTriggerReason => write!(
            f,
            "legacy-store fail-closed: loss-governor halt has no typed trigger reason; forward-only recoverability requires runtime-path recovery"
        ),
        LossGovernorClockManualRecoveryRefusal::FutureDatedTrigger {
            trigger_observed_at_ns,
            now_ns,
        } => write!(
            f,
            "future-dated trigger check refused: trigger_observed_at_ns={trigger_observed_at_ns} is after now_ns={now_ns}"
        ),
        LossGovernorClockManualRecoveryRefusal::StaleEvidence {
            evidence_observed_at_ns,
            trigger_observed_at_ns,
        } => write!(
            f,
            "stale evidence check refused: evidence observed_at_ns={evidence_observed_at_ns} is before trigger observed_at_ns={trigger_observed_at_ns}"
        ),
        LossGovernorClockManualRecoveryRefusal::FutureDatedEvidence {
            evidence_observed_at_ns,
            now_ns,
        } => write!(
            f,
            "future-dated evidence check refused: evidence observed_at_ns={evidence_observed_at_ns} is after now_ns={now_ns}"
        ),
        LossGovernorClockManualRecoveryRefusal::DailyWindowStillOpen {
            trigger_observed_at_ns,
            now_ns,
        } => write!(
            f,
            "daily_loss_limit refused: triggering UTC day has not rolled; trigger_observed_at_ns={trigger_observed_at_ns} now_ns={now_ns}"
        ),
        LossGovernorClockManualRecoveryRefusal::RollingWindowStillOpen {
            trigger_observed_at_ns,
            now_ns,
            rolling_window_ns,
        } => write!(
            f,
            "rolling_loss_limit refused: more than the full CURRENT config rolling window must have elapsed and exact-equality refuses; trigger_observed_at_ns={trigger_observed_at_ns} now_ns={now_ns} rolling_window_ns={rolling_window_ns}"
        ),
        LossGovernorClockManualRecoveryRefusal::RuntimePathRequired { trigger_reason } => write!(
            f,
            "{} requires runtime-path recovery; tracked follow-up for offline recovery",
            trigger_reason.as_str()
        ),
        LossGovernorClockManualRecoveryRefusal::RecoveryMode { recovery_mode } => write!(
            f,
            "recovery-mode check refused: recovery_mode={recovery_mode:?} is not supported for clock-verified manual recovery"
        ),
        LossGovernorClockManualRecoveryRefusal::InvalidEvidence => write!(
            f,
            "invalid evidence check refused: manual-recovery evidence is missing or structurally invalid"
        ),
        LossGovernorClockManualRecoveryRefusal::InvalidRollingWindow => write!(
            f,
            "rolling_loss_limit refused: configured rolling window must be positive"
        ),
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
            Self::RepairManualRecoveryAudit { path } => write!(
                f,
                "repair-the-audit-file-and-retry: loss-governor manual recovery audit file {} has a torn final line; halt state was left untouched",
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

    let store = KillSwitchStore::from_root_config_path(&loaded.root_path, kill_switch);
    let action_policy = halt_action_policy(loss_governor)?;
    let evidence = match LossGovernorManualRecoveryEvidence::new(
        command.operator_id.clone(),
        command.evidence_path.clone(),
        command.evidence_sha256.clone(),
        command.observed_at_ns,
        action_policy.manual_recovery_evidence_max_path_bytes,
    ) {
        Ok(evidence) => evidence,
        Err(error) => {
            record_refused_manual_recovery_command_attempt(
                &store,
                &command,
                command.now_ns,
                &format!("invalid manual recovery evidence: {error:?}"),
            )?;
            return Err(LossGovernorManualRecoveryError::InvalidManualRecoveryEvidence(error));
        }
    };
    if !kill_switch
        .authorized_operator_ids
        .iter()
        .any(|operator_id| operator_id == evidence.operator_id())
    {
        record_refused_manual_recovery_attempt(
            &store,
            &evidence,
            command.now_ns,
            "authorization refused: operator is not authorized by risk.kill_switch.authorized_operator_ids",
        )?;
        return Err(LossGovernorManualRecoveryError::UnauthorizedOperator {
            operator_id: evidence.operator_id().to_string(),
        });
    }

    let record = match store.load_recovery_record() {
        Ok(record) => record,
        Err(error) => {
            let error = LossGovernorManualRecoveryError::StoreLoad(error);
            record_refused_manual_recovery_attempt(
                &store,
                &evidence,
                command.now_ns,
                &error.to_string(),
            )?;
            return Err(error);
        }
    };
    let current_state = match recoverable_current_state(&record, store.path().to_path_buf()) {
        Ok(current_state) => current_state,
        Err(error) => {
            record_refused_manual_recovery_attempt(
                &store,
                &evidence,
                command.now_ns,
                &error.to_string(),
            )?;
            return Err(error);
        }
    };
    let recoverable_halt = match loss_governor_recoverable_halt(&current_state) {
        Ok(recoverable_halt) => recoverable_halt,
        Err(error) => {
            record_refused_manual_recovery_attempt(
                &store,
                &evidence,
                command.now_ns,
                &error.to_string(),
            )?;
            return Err(error);
        }
    };
    let loss_protection = match record.loss_protection.as_ref() {
        Some(loss_protection) => loss_protection,
        None => {
            let error = LossGovernorManualRecoveryError::MissingLossProtectionSnapshot {
                path: store.path().to_path_buf(),
            };
            record_refused_manual_recovery_attempt(
                &store,
                &evidence,
                command.now_ns,
                &error.to_string(),
            )?;
            return Err(error);
        }
    };

    if let Err(reason) = next_loss_governor_clock_verified_manual_recovery_trading_state(
        LossGovernorClockManualRecoveryRequest {
            policy: &action_policy,
            current_state: recoverable_halt.current_trading_state,
            trigger_reason: recoverable_halt.trigger_reason,
            trigger_observed_at_ns: recoverable_halt.trigger_observed_at_ns,
            now_ns: command.now_ns,
            rolling_window_ns: loss_governor.rolling_window_ns,
            evidence: Some(&evidence),
            max_evidence_path_bytes: action_policy.manual_recovery_evidence_max_path_bytes,
        },
    ) {
        let refusal = LossGovernorManualRecoveryRefusal::TriggerClock(reason);
        record_refused_manual_recovery_attempt(
            &store,
            &evidence,
            command.now_ns,
            &refusal.to_string(),
        )?;
        return Err(LossGovernorManualRecoveryError::RecoveryRefused { reason: refusal });
    }

    let previous_state = current_state.kind();
    let manual_recovery = manual_recovery_record(
        &evidence,
        command.now_ns,
        KillSwitchLossGovernorManualRecoveryOutcome::Attempted,
        None,
    );
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
    let write_failed_manual_recovery = manual_recovery.clone();
    let recovered_manual_recovery = KillSwitchLossGovernorManualRecoveryRecord {
        outcome: Some(KillSwitchLossGovernorManualRecoveryOutcome::Recovered),
        outcome_reason: None,
        ..manual_recovery.clone()
    };
    let manual_recovery_count = match store.append_loss_governor_manual_recovery(manual_recovery) {
        Ok(count) => count,
        Err(recovery_error) => {
            if let Some(error) = repair_manual_recovery_audit_error(&recovery_error) {
                return Err(error);
            }
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
        let write_failed = KillSwitchLossGovernorManualRecoveryRecord {
            outcome: Some(KillSwitchLossGovernorManualRecoveryOutcome::WriteFailed),
            outcome_reason: Some(format!(
                "loss governor manual recovery write failed: {recovery_error:?}"
            )),
            ..write_failed_manual_recovery
        };
        if let Err(audit_error) = store.append_loss_governor_manual_recovery(write_failed) {
            if let Some(error) = repair_manual_recovery_audit_error(&audit_error) {
                return Err(error);
            }
            log::error!(
                "failed to append write-failed loss-governor manual recovery audit line for {}: {audit_error}",
                store.path().display()
            );
        }
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
    match store.append_loss_governor_manual_recovery(recovered_manual_recovery) {
        Ok(count) => Ok(count),
        Err(audit_error) => {
            log::error!(
                "failed to append recovered loss-governor manual recovery audit line for {} after Armed state write succeeded: {audit_error}",
                store.path().display()
            );
            Ok(manual_recovery_count)
        }
    }
}

fn record_refused_manual_recovery_attempt(
    store: &impl LossGovernorManualRecoveryStoreWriter,
    evidence: &LossGovernorManualRecoveryEvidence,
    recorded_at_ns: u64,
    refusal: &str,
) -> Result<(), LossGovernorManualRecoveryError> {
    let refused = manual_recovery_record(
        evidence,
        recorded_at_ns,
        KillSwitchLossGovernorManualRecoveryOutcome::RefusedWithReason,
        Some(refusal.to_string()),
    );
    append_refused_manual_recovery_record(store, refused)
}

fn record_refused_manual_recovery_command_attempt(
    store: &impl LossGovernorManualRecoveryStoreWriter,
    command: &LossGovernorManualRecoveryCommand,
    recorded_at_ns: u64,
    refusal: &str,
) -> Result<(), LossGovernorManualRecoveryError> {
    let refused = KillSwitchLossGovernorManualRecoveryRecord {
        operator_id: command.operator_id.clone(),
        evidence_path: command.evidence_path.clone(),
        evidence_sha256: command.evidence_sha256.clone(),
        observed_at_ns: command.observed_at_ns,
        recorded_at_ns,
        outcome: Some(KillSwitchLossGovernorManualRecoveryOutcome::RefusedWithReason),
        outcome_reason: Some(refusal.to_string()),
    };
    append_refused_manual_recovery_record(store, refused)
}

fn append_refused_manual_recovery_record(
    store: &impl LossGovernorManualRecoveryStoreWriter,
    refused: KillSwitchLossGovernorManualRecoveryRecord,
) -> Result<(), LossGovernorManualRecoveryError> {
    match store.append_loss_governor_manual_recovery(refused) {
        Ok(_) => Ok(()),
        Err(error) => {
            if let Some(error) = repair_manual_recovery_audit_error(&error) {
                return Err(error);
            }
            log::error!(
                "failed to append refused loss-governor manual recovery audit line for {}: {error}",
                store.path().display()
            );
            Ok(())
        }
    }
}

fn repair_manual_recovery_audit_error(
    error: &KillSwitchStoreError,
) -> Option<LossGovernorManualRecoveryError> {
    match error {
        KillSwitchStoreError::TornManualRecoveryAudit { path } => {
            Some(LossGovernorManualRecoveryError::RepairManualRecoveryAudit { path: path.clone() })
        }
        _ => None,
    }
}

fn manual_recovery_record(
    evidence: &LossGovernorManualRecoveryEvidence,
    recorded_at_ns: u64,
    outcome: KillSwitchLossGovernorManualRecoveryOutcome,
    outcome_reason: Option<String>,
) -> KillSwitchLossGovernorManualRecoveryRecord {
    KillSwitchLossGovernorManualRecoveryRecord {
        operator_id: evidence.operator_id().to_string(),
        evidence_path: evidence.evidence_path().to_string(),
        evidence_sha256: evidence.evidence_sha256().to_string(),
        observed_at_ns: evidence.observed_at_ns(),
        recorded_at_ns,
        outcome: Some(outcome),
        outcome_reason,
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecoverableLossGovernorHalt {
    current_trading_state: TradingState,
    trigger_reason: Option<LossHaltReason>,
    trigger_observed_at_ns: u64,
}

fn loss_governor_recoverable_halt(
    state: &KillSwitchState,
) -> Result<RecoverableLossGovernorHalt, LossGovernorManualRecoveryError> {
    match state {
        KillSwitchState::Halting { trigger, .. } | KillSwitchState::Halted { trigger, .. } => {
            if trigger.kind == KillSwitchHaltTriggerKind::LossGovernorBreach {
                Ok(RecoverableLossGovernorHalt {
                    current_trading_state: TradingState::Reducing,
                    trigger_reason: trigger.loss_halt_reason,
                    trigger_observed_at_ns: trigger.source_timestamp_unix_nanos,
                })
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
    use rust_decimal::Decimal;

    enum FakeAppendResult {
        Ok(usize),
        Io,
        TornAudit,
    }

    struct FakeManualRecoveryStore {
        path: PathBuf,
        append_calls: RefCell<usize>,
        appended_records: RefCell<Vec<KillSwitchLossGovernorManualRecoveryRecord>>,
        append_results: RefCell<VecDeque<FakeAppendResult>>,
        write_results: RefCell<VecDeque<bool>>,
        written_states: RefCell<Vec<KillSwitchState>>,
    }

    impl FakeManualRecoveryStore {
        fn new(write_results: impl IntoIterator<Item = bool>) -> Self {
            Self::with_append_results(write_results, [])
        }

        fn with_append_results(
            write_results: impl IntoIterator<Item = bool>,
            append_results: impl IntoIterator<Item = FakeAppendResult>,
        ) -> Self {
            Self {
                path: PathBuf::from("state/kill-switch.json"),
                append_calls: RefCell::new(0),
                appended_records: RefCell::new(Vec::new()),
                append_results: RefCell::new(append_results.into_iter().collect()),
                write_results: RefCell::new(write_results.into_iter().collect()),
                written_states: RefCell::new(Vec::new()),
            }
        }

        fn appended_records(&self) -> Vec<KillSwitchLossGovernorManualRecoveryRecord> {
            self.appended_records.borrow().clone()
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
            manual_recovery: KillSwitchLossGovernorManualRecoveryRecord,
        ) -> Result<usize, KillSwitchStoreError> {
            *self.append_calls.borrow_mut() += 1;
            self.appended_records.borrow_mut().push(manual_recovery);
            match self.append_results.borrow_mut().pop_front() {
                Some(FakeAppendResult::Ok(count)) => Ok(count),
                None => Ok(1),
                Some(FakeAppendResult::Io) => Err(synthetic_store_error(&self.path)),
                Some(FakeAppendResult::TornAudit) => {
                    Err(KillSwitchStoreError::TornManualRecoveryAudit {
                        path: self
                            .path
                            .with_file_name("kill-switch-manual-recoveries.jsonl"),
                    })
                }
            }
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
            outcome: Some(KillSwitchLossGovernorManualRecoveryOutcome::Attempted),
            outcome_reason: None,
        }
    }

    #[test]
    fn attempted_audit_non_torn_error_persists_failed_manual_intervention() {
        let store = FakeManualRecoveryStore::with_append_results([true], [FakeAppendResult::Io]);

        let error = persist_manual_recovery_attempt(
            &store,
            &recovery_state(),
            &loss_snapshot(),
            manual_recovery_record(),
        )
        .expect_err("attempted audit IO failure should persist failed intervention");

        assert!(matches!(
            error,
            LossGovernorManualRecoveryError::StoreWriteFailed { .. }
        ));
        assert_eq!(*store.append_calls.borrow(), 1);
        let written_states = store.written_states();
        assert_eq!(written_states.len(), 1);
        assert!(matches!(
            written_states[0],
            KillSwitchState::FailedManualIntervention { .. }
        ));
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
        assert_eq!(*store.append_calls.borrow(), 2);
        let appended_records = store.appended_records();
        assert_eq!(
            appended_records[0].outcome,
            Some(KillSwitchLossGovernorManualRecoveryOutcome::Attempted)
        );
        assert_eq!(
            appended_records[1].outcome,
            Some(KillSwitchLossGovernorManualRecoveryOutcome::WriteFailed)
        );
        let written_states = store.written_states();
        assert_eq!(written_states.len(), 2);
        assert_eq!(written_states[0], KillSwitchState::Armed);
        assert!(matches!(
            written_states[1],
            KillSwitchState::FailedManualIntervention { .. }
        ));
    }

    #[test]
    fn write_failed_audit_io_error_persists_failed_manual_intervention() {
        let store = FakeManualRecoveryStore::with_append_results(
            [false, true],
            [FakeAppendResult::Ok(1), FakeAppendResult::Io],
        );

        let error = persist_manual_recovery_attempt(
            &store,
            &recovery_state(),
            &loss_snapshot(),
            manual_recovery_record(),
        )
        .expect_err("non-torn write-failed audit error should still downgrade failed write");

        assert!(matches!(
            error,
            LossGovernorManualRecoveryError::StoreWriteFailed { .. }
        ));
        assert_eq!(*store.append_calls.borrow(), 2);
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
        assert_eq!(*store.append_calls.borrow(), 2);
        let appended_records = store.appended_records();
        assert_eq!(
            appended_records[0].outcome,
            Some(KillSwitchLossGovernorManualRecoveryOutcome::Attempted)
        );
        assert_eq!(
            appended_records[1].outcome,
            Some(KillSwitchLossGovernorManualRecoveryOutcome::WriteFailed)
        );
        let written_states = store.written_states();
        assert_eq!(written_states.len(), 2);
        assert_eq!(written_states[0], KillSwitchState::Armed);
        assert!(matches!(
            written_states[1],
            KillSwitchState::FailedManualIntervention { .. }
        ));
    }

    #[test]
    fn failed_recovery_state_write_with_torn_write_failed_audit_requires_repair_only() {
        let store = FakeManualRecoveryStore::with_append_results(
            [false],
            [FakeAppendResult::Ok(1), FakeAppendResult::TornAudit],
        );

        let error = persist_manual_recovery_attempt(
            &store,
            &recovery_state(),
            &loss_snapshot(),
            manual_recovery_record(),
        )
        .expect_err("torn write-failed audit line should require audit repair");

        assert!(
            matches!(
                error,
                LossGovernorManualRecoveryError::RepairManualRecoveryAudit { .. }
            ),
            "torn write-failed audit append should not downgrade the halt, got: {error}"
        );
        assert_eq!(*store.append_calls.borrow(), 2);
        let appended_records = store.appended_records();
        assert_eq!(
            appended_records[0].outcome,
            Some(KillSwitchLossGovernorManualRecoveryOutcome::Attempted)
        );
        assert_eq!(
            appended_records[1].outcome,
            Some(KillSwitchLossGovernorManualRecoveryOutcome::WriteFailed)
        );
        let written_states = store.written_states();
        assert_eq!(written_states.len(), 1);
        assert_eq!(written_states[0], KillSwitchState::Armed);
        assert!(
            !written_states
                .iter()
                .any(|state| matches!(state, KillSwitchState::FailedManualIntervention { .. })),
            "torn write-failed audit must leave the original halt latched: {written_states:?}"
        );
    }

    #[test]
    fn recovered_audit_torn_error_reports_success_after_armed_state_wins() {
        let store = FakeManualRecoveryStore::with_append_results(
            [true],
            [FakeAppendResult::Ok(1), FakeAppendResult::TornAudit],
        );

        let count = persist_manual_recovery_attempt(
            &store,
            &recovery_state(),
            &loss_snapshot(),
            manual_recovery_record(),
        )
        .expect("recovered audit torn error should not reverse durable Armed state");

        assert_eq!(count, 1);
        assert_eq!(*store.append_calls.borrow(), 2);
        assert_eq!(store.written_states(), vec![KillSwitchState::Armed]);
    }

    #[test]
    fn recovered_audit_non_torn_error_reports_success_after_armed_state_wins() {
        let store = FakeManualRecoveryStore::with_append_results(
            [true],
            [FakeAppendResult::Ok(1), FakeAppendResult::Io],
        );

        let count = persist_manual_recovery_attempt(
            &store,
            &recovery_state(),
            &loss_snapshot(),
            manual_recovery_record(),
        )
        .expect("recovered audit IO error should not reverse durable Armed state");

        assert_eq!(count, 1);
        assert_eq!(*store.append_calls.borrow(), 2);
        assert_eq!(store.written_states(), vec![KillSwitchState::Armed]);
    }
}
