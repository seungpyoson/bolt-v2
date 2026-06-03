use bolt_v2::bolt_v3_kill_switch::{
    KillSwitchEvent, KillSwitchEventKind, KillSwitchHaltTrigger, KillSwitchManualResetEvidence,
    KillSwitchManualResetEvidenceError, KillSwitchState, KillSwitchStateKind,
    KillSwitchTransitionContext, KillSwitchTransitionError, transition_kill_switch_state,
};

fn valid_manual_reset_evidence() -> KillSwitchManualResetEvidence {
    KillSwitchManualResetEvidence::new(
        "operator-primary",
        "reset/operator-primary.json",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        1_717_200_000_000_000_001,
    )
    .expect("manual reset evidence should be valid")
}

fn blocked_context() -> KillSwitchTransitionContext {
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

#[test]
fn loss_governor_trigger_moves_armed_to_halting_and_requires_durable_halt_evidence() {
    let trigger = KillSwitchHaltTrigger::loss_governor_breach(
        "bolt_v3.loss_governor",
        1_717_200_000_000_000_000,
        "daily_realized_loss_limit",
    );

    let halting = transition_kill_switch_state(
        KillSwitchState::Armed,
        KillSwitchEvent::HaltTriggered(trigger),
        blocked_context(),
    )
    .expect("loss-governor breach should latch halt");

    assert!(matches!(halting, KillSwitchState::Halting { .. }));

    let rejected = transition_kill_switch_state(
        halting,
        KillSwitchEvent::DurableHaltEvidenceRecorded,
        KillSwitchTransitionContext {
            state_write_succeeded: false,
            durable_halt_evidence_recorded: false,
            ..blocked_context()
        },
    );

    assert_eq!(
        rejected,
        Err(KillSwitchTransitionError::MissingDurableHaltEvidence)
    );
}

#[test]
fn durable_halt_evidence_write_failure_enters_failed_manual_intervention() {
    let halting = KillSwitchState::Halting {
        halt_id: "halt-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "bolt_v3.loss_governor",
            1_717_200_000_000_000_000,
            "daily_realized_loss_limit",
        ),
    };

    let failed = transition_kill_switch_state(
        halting,
        KillSwitchEvent::DurableHaltEvidenceWriteFailed {
            reason: "fsync failed".to_string(),
        },
        blocked_context(),
    )
    .expect("durable halt write failure should fail closed into manual intervention");

    assert_eq!(
        failed,
        KillSwitchState::FailedManualIntervention {
            halt_id: "halt-1".to_string(),
            reason: "fsync failed".to_string(),
        }
    );
}

#[test]
fn manual_reset_requires_authorization_evidence_and_fresh_clean_proof() {
    let flat = KillSwitchState::Flat {
        halt_id: "halt-1".to_string(),
    };
    let failed = KillSwitchState::FailedManualIntervention {
        halt_id: "halt-1".to_string(),
        reason: "fsync failed".to_string(),
    };

    for state in [flat, failed] {
        let rejected = transition_kill_switch_state(
            state,
            KillSwitchEvent::ManualResetRequested(valid_manual_reset_evidence()),
            KillSwitchTransitionContext {
                operator_authorized: false,
                manual_reset_evidence_valid: true,
                mandatory_proof_streams_fresh: true,
                no_outstanding_order_risk: true,
                no_open_positions: true,
                no_pending_entry_risk: true,
                ..blocked_context()
            },
        );

        assert_eq!(
            rejected,
            Err(KillSwitchTransitionError::UnauthorizedManualReset)
        );
    }
}

#[test]
fn halted_state_cannot_rearm_without_first_becoming_flat() {
    let halted = KillSwitchState::Halted {
        halt_id: "halt-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "bolt_v3.loss_governor",
            1_717_200_000_000_000_000,
            "daily_realized_loss_limit",
        ),
    };

    let rejected = transition_kill_switch_state(
        halted,
        KillSwitchEvent::ManualResetRequested(valid_manual_reset_evidence()),
        KillSwitchTransitionContext {
            operator_authorized: true,
            manual_reset_evidence_valid: true,
            mandatory_proof_streams_fresh: true,
            no_outstanding_order_risk: true,
            no_open_positions: true,
            no_pending_entry_risk: true,
            ..blocked_context()
        },
    );

    assert_eq!(
        rejected,
        Err(KillSwitchTransitionError::IllegalTransition {
            state: KillSwitchStateKind::Halted,
            event: KillSwitchEventKind::ManualResetRequested,
        })
    );
}

#[test]
fn authorized_manual_reset_still_requires_valid_evidence_and_clean_proof() {
    let flat = KillSwitchState::Flat {
        halt_id: "halt-1".to_string(),
    };
    let valid_reset_context = KillSwitchTransitionContext {
        operator_authorized: true,
        manual_reset_evidence_valid: true,
        mandatory_proof_streams_fresh: true,
        no_outstanding_order_risk: true,
        no_open_positions: true,
        no_pending_entry_risk: true,
        ..blocked_context()
    };

    for (context, expected) in [
        (
            KillSwitchTransitionContext {
                manual_reset_evidence_valid: false,
                ..valid_reset_context
            },
            KillSwitchTransitionError::InvalidManualResetEvidence,
        ),
        (
            KillSwitchTransitionContext {
                mandatory_proof_streams_fresh: false,
                ..valid_reset_context
            },
            KillSwitchTransitionError::MissingFreshReconciliationProof,
        ),
        (
            KillSwitchTransitionContext {
                no_outstanding_order_risk: false,
                ..valid_reset_context
            },
            KillSwitchTransitionError::ReconciliationNotFlat,
        ),
        (
            KillSwitchTransitionContext {
                no_open_positions: false,
                ..valid_reset_context
            },
            KillSwitchTransitionError::ReconciliationNotFlat,
        ),
        (
            KillSwitchTransitionContext {
                no_pending_entry_risk: false,
                ..valid_reset_context
            },
            KillSwitchTransitionError::ReconciliationNotFlat,
        ),
    ] {
        let rejected = transition_kill_switch_state(
            flat.clone(),
            KillSwitchEvent::ManualResetRequested(valid_manual_reset_evidence()),
            context,
        );

        assert_eq!(rejected, Err(expected));
    }

    let armed = transition_kill_switch_state(
        flat,
        KillSwitchEvent::ManualResetRequested(valid_manual_reset_evidence()),
        valid_reset_context,
    )
    .expect("authorized reset with clean proof should re-arm");

    assert_eq!(armed, KillSwitchState::Armed);
}

#[test]
fn failed_manual_intervention_can_rearm_with_authorized_evidence_and_clean_proof() {
    let failed = KillSwitchState::FailedManualIntervention {
        halt_id: "halt-1".to_string(),
        reason: "fsync failed".to_string(),
    };

    let armed = transition_kill_switch_state(
        failed,
        KillSwitchEvent::ManualResetRequested(valid_manual_reset_evidence()),
        KillSwitchTransitionContext {
            operator_authorized: true,
            manual_reset_evidence_valid: true,
            mandatory_proof_streams_fresh: true,
            no_outstanding_order_risk: true,
            no_open_positions: true,
            no_pending_entry_risk: true,
            ..blocked_context()
        },
    )
    .expect("failed manual intervention should be resettable with full proof");

    assert_eq!(armed, KillSwitchState::Armed);
}

#[test]
fn manual_reset_event_carries_operator_identity_evidence_path_and_hash() {
    let evidence = valid_manual_reset_evidence();
    assert_eq!(evidence.operator_id(), "operator-primary");
    assert_eq!(evidence.evidence_path(), "reset/operator-primary.json");
    assert_eq!(
        evidence.evidence_sha256(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(
        evidence.requested_at_unix_nanos(),
        1_717_200_000_000_000_001
    );

    assert_eq!(
        KillSwitchManualResetEvidence::new(
            "",
            "reset/operator-primary.json",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            1,
        ),
        Err(KillSwitchManualResetEvidenceError::MissingOperatorId)
    );
    assert_eq!(
        KillSwitchManualResetEvidence::new(
            "operator-primary",
            "../reset.json",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            1,
        ),
        Err(KillSwitchManualResetEvidenceError::InvalidEvidencePath)
    );
    assert_eq!(
        KillSwitchManualResetEvidence::new("operator-primary", "reset.json", "not-sha256", 1),
        Err(KillSwitchManualResetEvidenceError::InvalidEvidenceSha256)
    );
}

#[test]
fn manual_reset_evidence_stores_trimmed_identity_path_and_hash() {
    let evidence = KillSwitchManualResetEvidence::new(
        " operator-primary ",
        " reset/operator-primary.json ",
        " aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ",
        1,
    )
    .expect("trimmed manual reset evidence should be valid");

    assert_eq!(evidence.operator_id(), "operator-primary");
    assert_eq!(evidence.evidence_path(), "reset/operator-primary.json");
    assert_eq!(
        evidence.evidence_sha256(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}

#[test]
fn reconciliation_requires_fresh_proof_and_no_remaining_risk_before_flat() {
    let halted = KillSwitchState::Halted {
        halt_id: "halt-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "bolt_v3.loss_governor",
            1_717_200_000_000_000_000,
            "daily_realized_loss_limit",
        ),
    };
    let clean_context = KillSwitchTransitionContext {
        mandatory_proof_streams_fresh: true,
        no_outstanding_order_risk: true,
        no_open_positions: true,
        no_pending_entry_risk: true,
        ..blocked_context()
    };

    for (context, expected) in [
        (
            KillSwitchTransitionContext {
                mandatory_proof_streams_fresh: false,
                ..clean_context
            },
            KillSwitchTransitionError::MissingFreshReconciliationProof,
        ),
        (
            KillSwitchTransitionContext {
                no_outstanding_order_risk: false,
                ..clean_context
            },
            KillSwitchTransitionError::ReconciliationNotFlat,
        ),
        (
            KillSwitchTransitionContext {
                no_open_positions: false,
                ..clean_context
            },
            KillSwitchTransitionError::ReconciliationNotFlat,
        ),
        (
            KillSwitchTransitionContext {
                no_pending_entry_risk: false,
                ..clean_context
            },
            KillSwitchTransitionError::ReconciliationNotFlat,
        ),
    ] {
        let rejected = transition_kill_switch_state(
            halted.clone(),
            KillSwitchEvent::ReconciliationProofReceived,
            context,
        );

        assert_eq!(rejected, Err(expected));
    }

    let flat = transition_kill_switch_state(
        halted,
        KillSwitchEvent::ReconciliationProofReceived,
        clean_context,
    )
    .expect("fresh clean reconciliation proof should mark halt flat");

    assert_eq!(
        flat,
        KillSwitchState::Flat {
            halt_id: "halt-1".to_string()
        }
    );
}

#[test]
fn phase3_cancel_and_flatten_states_do_not_shortcut_to_flat_or_armed() {
    let clean_context = KillSwitchTransitionContext {
        operator_authorized: true,
        manual_reset_evidence_valid: true,
        mandatory_proof_streams_fresh: true,
        no_outstanding_order_risk: true,
        no_open_positions: true,
        no_pending_entry_risk: true,
        ..blocked_context()
    };

    for (state, kind) in [
        (
            KillSwitchState::Cancelling {
                halt_id: "halt-1".to_string(),
            },
            KillSwitchStateKind::Cancelling,
        ),
        (
            KillSwitchState::Flattening {
                halt_id: "halt-1".to_string(),
            },
            KillSwitchStateKind::Flattening,
        ),
    ] {
        assert_eq!(
            transition_kill_switch_state(
                state.clone(),
                KillSwitchEvent::ReconciliationProofReceived,
                clean_context,
            ),
            Err(KillSwitchTransitionError::IllegalTransition {
                state: kind,
                event: KillSwitchEventKind::ReconciliationProofReceived,
            })
        );

        assert_eq!(
            transition_kill_switch_state(
                state,
                KillSwitchEvent::ManualResetRequested(valid_manual_reset_evidence()),
                clean_context,
            ),
            Err(KillSwitchTransitionError::IllegalTransition {
                state: kind,
                event: KillSwitchEventKind::ManualResetRequested,
            })
        );
    }
}
