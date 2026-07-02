use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path};

const HALT_ID_DOMAIN: &[u8] = b"bolt_v3.kill_switch.halt_id.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KillSwitchState {
    Armed,
    Halting {
        halt_id: String,
        trigger: KillSwitchHaltTrigger,
    },
    Halted {
        halt_id: String,
        trigger: KillSwitchHaltTrigger,
    },
    Cancelling {
        halt_id: String,
    },
    Flattening {
        halt_id: String,
    },
    Flat {
        halt_id: String,
    },
    FailedManualIntervention {
        halt_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KillSwitchEvent {
    HaltTriggered(KillSwitchHaltTrigger),
    DurableHaltEvidenceRecorded,
    DurableHaltEvidenceWriteFailed { reason: String },
    HaltActionDispatchFailed { reason: String },
    ReconciliationProofReceived,
    ManualResetRequested(KillSwitchManualResetEvidence),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillSwitchHaltTrigger {
    pub kind: KillSwitchHaltTriggerKind,
    pub source: String,
    pub source_timestamp_unix_nanos: u64,
    pub reason: String,
}

impl KillSwitchHaltTrigger {
    pub fn loss_governor_breach(
        source: impl Into<String>,
        source_timestamp_unix_nanos: u64,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind: KillSwitchHaltTriggerKind::LossGovernorBreach,
            source: source.into(),
            source_timestamp_unix_nanos,
            reason: reason.into(),
        }
    }

    pub fn basket_execution_stuck(
        source: impl Into<String>,
        source_timestamp_unix_nanos: u64,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind: KillSwitchHaltTriggerKind::BasketExecutionStuck,
            source: source.into(),
            source_timestamp_unix_nanos,
            reason: reason.into(),
        }
    }

    pub fn venue_truth_divergence(
        source: impl Into<String>,
        source_timestamp_unix_nanos: u64,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind: KillSwitchHaltTriggerKind::VenueTruthDivergence,
            source: source.into(),
            source_timestamp_unix_nanos,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KillSwitchHaltTriggerKind {
    LossGovernorBreach,
    BasketExecutionStuck,
    VenueTruthDivergence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillSwitchManualResetEvidence {
    operator_id: String,
    evidence_path: String,
    evidence_sha256: String,
    requested_at_unix_nanos: u64,
}

impl KillSwitchManualResetEvidence {
    pub fn new(
        operator_id: impl Into<String>,
        evidence_path: impl Into<String>,
        evidence_sha256: impl Into<String>,
        requested_at_unix_nanos: u64,
    ) -> Result<Self, KillSwitchManualResetEvidenceError> {
        let operator_id = operator_id.into().trim().to_string();
        let evidence_path = evidence_path.into().trim().to_string();
        let evidence_sha256 = evidence_sha256.into().trim().to_string();

        if operator_id.is_empty() {
            return Err(KillSwitchManualResetEvidenceError::MissingOperatorId);
        }
        let evidence_path_value = Path::new(&evidence_path);
        if evidence_path_value.as_os_str().is_empty()
            || evidence_path_value.is_absolute()
            || evidence_path_value
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(KillSwitchManualResetEvidenceError::InvalidEvidencePath);
        }
        if evidence_sha256.len() != 64
            || !evidence_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(KillSwitchManualResetEvidenceError::InvalidEvidenceSha256);
        }

        Ok(Self {
            operator_id,
            evidence_path,
            evidence_sha256,
            requested_at_unix_nanos,
        })
    }

    pub fn operator_id(&self) -> &str {
        &self.operator_id
    }

    pub fn evidence_path(&self) -> &str {
        &self.evidence_path
    }

    pub fn evidence_sha256(&self) -> &str {
        &self.evidence_sha256
    }

    pub fn requested_at_unix_nanos(&self) -> u64 {
        self.requested_at_unix_nanos
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSwitchManualResetEvidenceError {
    MissingOperatorId,
    InvalidEvidencePath,
    InvalidEvidenceSha256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KillSwitchTransitionContext {
    pub state_write_succeeded: bool,
    pub durable_halt_evidence_recorded: bool,
    pub operator_authorized: bool,
    pub manual_reset_evidence_valid: bool,
    pub mandatory_proof_streams_fresh: bool,
    pub no_outstanding_order_risk: bool,
    pub no_open_positions: bool,
    pub no_pending_entry_risk: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillSwitchTransitionError {
    MissingDurableHaltEvidence,
    UnauthorizedManualReset,
    InvalidManualResetEvidence,
    MissingFreshReconciliationProof,
    ReconciliationNotFlat,
    IllegalTransition {
        state: KillSwitchStateKind,
        event: KillSwitchEventKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSwitchStateKind {
    Armed,
    Halting,
    Halted,
    Cancelling,
    Flattening,
    Flat,
    FailedManualIntervention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSwitchEventKind {
    HaltTriggered,
    DurableHaltEvidenceRecorded,
    DurableHaltEvidenceWriteFailed,
    HaltActionDispatchFailed,
    ReconciliationProofReceived,
    ManualResetRequested,
}

pub fn transition_kill_switch_state(
    state: KillSwitchState,
    event: KillSwitchEvent,
    context: KillSwitchTransitionContext,
) -> Result<KillSwitchState, KillSwitchTransitionError> {
    match (state, event) {
        (KillSwitchState::Armed, KillSwitchEvent::HaltTriggered(trigger)) => {
            let halt_id = halt_id_for_trigger(&trigger);
            Ok(KillSwitchState::Halting { halt_id, trigger })
        }
        (
            KillSwitchState::Halting { halt_id, trigger },
            KillSwitchEvent::DurableHaltEvidenceRecorded,
        ) => {
            if !context.state_write_succeeded || !context.durable_halt_evidence_recorded {
                return Err(KillSwitchTransitionError::MissingDurableHaltEvidence);
            }
            Ok(KillSwitchState::Halted { halt_id, trigger })
        }
        (
            KillSwitchState::Halting { halt_id, .. },
            KillSwitchEvent::DurableHaltEvidenceWriteFailed { reason },
        ) => Ok(KillSwitchState::FailedManualIntervention { halt_id, reason }),
        (
            KillSwitchState::Halting { halt_id, .. },
            KillSwitchEvent::HaltActionDispatchFailed { reason },
        ) => Ok(KillSwitchState::FailedManualIntervention { halt_id, reason }),
        (KillSwitchState::Halted { halt_id, .. }, KillSwitchEvent::ReconciliationProofReceived) => {
            require_fresh_clean_reconciliation(&context)?;
            Ok(KillSwitchState::Flat { halt_id })
        }
        (
            KillSwitchState::Flat { .. } | KillSwitchState::FailedManualIntervention { .. },
            KillSwitchEvent::ManualResetRequested(_evidence),
        ) => {
            if !context.operator_authorized {
                return Err(KillSwitchTransitionError::UnauthorizedManualReset);
            }
            if !context.manual_reset_evidence_valid {
                return Err(KillSwitchTransitionError::InvalidManualResetEvidence);
            }
            if !context.mandatory_proof_streams_fresh {
                return Err(KillSwitchTransitionError::MissingFreshReconciliationProof);
            }
            require_clean_reconciliation(&context)?;
            Ok(KillSwitchState::Armed)
        }
        (state, event) => Err(KillSwitchTransitionError::IllegalTransition {
            state: state.kind(),
            event: event.kind(),
        }),
    }
}

impl KillSwitchState {
    pub fn kind(&self) -> KillSwitchStateKind {
        match self {
            KillSwitchState::Armed => KillSwitchStateKind::Armed,
            KillSwitchState::Halting { .. } => KillSwitchStateKind::Halting,
            KillSwitchState::Halted { .. } => KillSwitchStateKind::Halted,
            KillSwitchState::Cancelling { .. } => KillSwitchStateKind::Cancelling,
            KillSwitchState::Flattening { .. } => KillSwitchStateKind::Flattening,
            KillSwitchState::Flat { .. } => KillSwitchStateKind::Flat,
            KillSwitchState::FailedManualIntervention { .. } => {
                KillSwitchStateKind::FailedManualIntervention
            }
        }
    }
}

impl KillSwitchEvent {
    fn kind(&self) -> KillSwitchEventKind {
        match self {
            KillSwitchEvent::HaltTriggered(_) => KillSwitchEventKind::HaltTriggered,
            KillSwitchEvent::DurableHaltEvidenceRecorded => {
                KillSwitchEventKind::DurableHaltEvidenceRecorded
            }
            KillSwitchEvent::DurableHaltEvidenceWriteFailed { .. } => {
                KillSwitchEventKind::DurableHaltEvidenceWriteFailed
            }
            KillSwitchEvent::HaltActionDispatchFailed { .. } => {
                KillSwitchEventKind::HaltActionDispatchFailed
            }
            KillSwitchEvent::ReconciliationProofReceived => {
                KillSwitchEventKind::ReconciliationProofReceived
            }
            KillSwitchEvent::ManualResetRequested(_) => KillSwitchEventKind::ManualResetRequested,
        }
    }
}

fn require_fresh_clean_reconciliation(
    context: &KillSwitchTransitionContext,
) -> Result<(), KillSwitchTransitionError> {
    if !context.mandatory_proof_streams_fresh {
        return Err(KillSwitchTransitionError::MissingFreshReconciliationProof);
    }
    require_clean_reconciliation(context)
}

fn require_clean_reconciliation(
    context: &KillSwitchTransitionContext,
) -> Result<(), KillSwitchTransitionError> {
    if !context.no_outstanding_order_risk
        || !context.no_open_positions
        || !context.no_pending_entry_risk
    {
        return Err(KillSwitchTransitionError::ReconciliationNotFlat);
    }
    Ok(())
}

fn halt_id_for_trigger(trigger: &KillSwitchHaltTrigger) -> String {
    let mut hasher = Sha256::new();
    hasher.update(HALT_ID_DOMAIN);
    hasher.update([0]);
    hasher.update(match trigger.kind {
        KillSwitchHaltTriggerKind::LossGovernorBreach => b"loss_governor_breach".as_slice(),
        KillSwitchHaltTriggerKind::BasketExecutionStuck => b"basket_execution_stuck".as_slice(),
        KillSwitchHaltTriggerKind::VenueTruthDivergence => b"venue_truth_divergence".as_slice(),
    });
    hasher.update([0]);
    hasher.update(trigger.source.as_bytes());
    hasher.update([0]);
    hasher.update(trigger.source_timestamp_unix_nanos.to_be_bytes());
    hasher.update([0]);
    hasher.update(trigger.reason.as_bytes());
    hex::encode(hasher.finalize())
}
