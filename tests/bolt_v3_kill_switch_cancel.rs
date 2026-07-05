use bolt_v2::{
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState},
    bolt_v3_kill_switch_cancel::{
        BoltV3KillSwitchCancelAggregateResult, BoltV3KillSwitchCancelAttemptOutcome,
        BoltV3KillSwitchCancelCandidate, BoltV3KillSwitchCancelDecisionMode,
        BoltV3KillSwitchCancelError, BoltV3KillSwitchCancelOutcomeAggregation,
        BoltV3KillSwitchCancelOutcomeEvidence, BoltV3KillSwitchCancelPlanRequest,
        BoltV3KillSwitchCancelPolicy, BoltV3KillSwitchCancelRouteKind,
        BoltV3KillSwitchCancelRouteProof, BoltV3KillSwitchCancelScope,
        BoltV3KillSwitchCancelSnapshot, BoltV3KillSwitchCancelSupervisor,
        BoltV3KillSwitchOutstandingOrderRiskSurface,
    },
};
use nautilus_model::{
    enums::OrderStatus,
    identifiers::{AccountId, ClientOrderId, InstrumentId, StrategyId},
};

const ACTION_ID: &str = "cancel-action-1";
const POLICY_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONFIG_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const REQUEST_SOURCE_TIMESTAMP_UNIX_NANOS: u64 = 1_717_200_000_000_000_001;
const REQUEST_OBSERVED_AT_UNIX_NANOS: u64 = 1_717_200_000_000_000_002;

#[test]
fn cancel_snapshot_covers_all_mandatory_outstanding_order_risk_surfaces() {
    let mandatory_surfaces = BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces();
    assert_eq!(
        mandatory_surfaces,
        &[
            BoltV3KillSwitchOutstandingOrderRiskSurface::Open,
            BoltV3KillSwitchOutstandingOrderRiskSurface::Inflight,
            BoltV3KillSwitchOutstandingOrderRiskSurface::PendingCancel,
            BoltV3KillSwitchOutstandingOrderRiskSurface::Emulated,
            BoltV3KillSwitchOutstandingOrderRiskSurface::AlgorithmManaged,
            BoltV3KillSwitchOutstandingOrderRiskSurface::Contingent,
            BoltV3KillSwitchOutstandingOrderRiskSurface::AcceptedButNotTerminal,
        ]
    );

    let candidates = mandatory_surfaces
        .iter()
        .enumerate()
        .map(|(index, surface)| cancel_candidate(*surface, &format!("client-order-{index}")))
        .collect::<Vec<_>>();

    let snapshot = BoltV3KillSwitchCancelSnapshot::new(candidates)
        .expect("complete cancel snapshot should be valid");

    assert!(snapshot.has_outstanding_risk());
    assert_eq!(snapshot.candidates().len(), mandatory_surfaces.len());
    assert_eq!(
        snapshot.missing_mandatory_surfaces(mandatory_surfaces),
        Vec::<BoltV3KillSwitchOutstandingOrderRiskSurface>::new()
    );
}

#[test]
fn cancel_snapshot_reports_missing_mandatory_surface_proof() {
    let mandatory_surfaces = BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces();
    let candidates = mandatory_surfaces
        .iter()
        .filter(|surface| **surface != BoltV3KillSwitchOutstandingOrderRiskSurface::Contingent)
        .enumerate()
        .map(|(index, surface)| cancel_candidate(*surface, &format!("client-order-{index}")))
        .collect::<Vec<_>>();

    let snapshot = BoltV3KillSwitchCancelSnapshot::new(candidates)
        .expect("partial cancel snapshot should still preserve observed risk");

    assert_eq!(
        snapshot.missing_mandatory_surfaces(mandatory_surfaces),
        vec![BoltV3KillSwitchOutstandingOrderRiskSurface::Contingent]
    );
}

#[test]
fn cancel_candidate_stores_nt_order_identity_and_status_fields() {
    let candidate = BoltV3KillSwitchCancelCandidate::from_nt_order_state(
        BoltV3KillSwitchOutstandingOrderRiskSurface::Open,
        account_id(),
        instrument_id(),
        strategy_id("binary-oracle-edge-taker-001"),
        client_order_id("client-order-1"),
        OrderStatus::Accepted,
        1_717_200_000_000_000_000,
    )
    .expect("NT-backed cancel candidate should be valid");

    assert_eq!(candidate.account_id(), account_id());
    assert_eq!(candidate.instrument_id(), instrument_id());
    assert_eq!(
        candidate.strategy_id(),
        strategy_id("binary-oracle-edge-taker-001")
    );
    assert_eq!(
        candidate.client_order_id(),
        client_order_id("client-order-1")
    );
    assert_eq!(candidate.order_status(), OrderStatus::Accepted);
}

#[test]
fn cancel_snapshot_deduplicates_scoped_order_identity_without_losing_surface_proof() {
    let candidates = vec![
        cancel_candidate(
            BoltV3KillSwitchOutstandingOrderRiskSurface::Open,
            "client-order-1",
        ),
        cancel_candidate(
            BoltV3KillSwitchOutstandingOrderRiskSurface::PendingCancel,
            "client-order-1",
        ),
        cancel_candidate_for_strategy(
            BoltV3KillSwitchOutstandingOrderRiskSurface::Open,
            "client-order-1",
            "binary-oracle-edge-taker-002",
        ),
    ];

    let snapshot = BoltV3KillSwitchCancelSnapshot::new(candidates)
        .expect("duplicate cancel snapshot should be valid");

    assert_eq!(snapshot.candidates().len(), 2);
    assert_eq!(
        snapshot.missing_mandatory_surfaces(&[
            BoltV3KillSwitchOutstandingOrderRiskSurface::Open,
            BoltV3KillSwitchOutstandingOrderRiskSurface::PendingCancel,
        ]),
        Vec::<BoltV3KillSwitchOutstandingOrderRiskSurface>::new()
    );
}

#[test]
fn cancel_policy_rejects_snapshot_missing_mandatory_surface_proof() {
    let policy = BoltV3KillSwitchCancelPolicy::new(
        BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces()
            .iter()
            .copied(),
    )
    .expect("mandatory surface policy should be valid");
    let candidates = BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces()
        .iter()
        .filter(|surface| **surface != BoltV3KillSwitchOutstandingOrderRiskSurface::Contingent)
        .enumerate()
        .map(|(index, surface)| cancel_candidate(*surface, &format!("client-order-{index}")))
        .collect::<Vec<_>>();
    let snapshot = BoltV3KillSwitchCancelSnapshot::new(candidates)
        .expect("partial cancel snapshot should preserve observed risk");

    assert_eq!(
        policy.validate_snapshot(&snapshot),
        Err(BoltV3KillSwitchCancelError::MissingMandatorySurfaceProof)
    );
}

#[test]
fn cancel_supervisor_planning_requires_cancelling_state() {
    for state in non_cancelling_states() {
        let error = BoltV3KillSwitchCancelSupervisor::plan_cancel(cancel_plan_request(state))
            .expect_err("cancel planning must reject non-cancelling durable states");
        assert_eq!(
            error,
            BoltV3KillSwitchCancelError::KillSwitchStateNotCancelling
        );
    }

    let plan =
        BoltV3KillSwitchCancelSupervisor::plan_cancel(cancel_plan_request(cancelling_state()))
            .expect("cancelling durable state should allow proof-only cancel planning");

    assert_eq!(plan.halt_id(), "halt-1");
    assert_eq!(
        plan.decision_mode(),
        BoltV3KillSwitchCancelDecisionMode::DryRunProofOnly
    );
    assert_eq!(
        plan.candidates().len(),
        BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces().len()
    );
}

#[test]
fn cancel_supervisor_commands_bind_request_and_candidate_metadata() {
    let plan =
        BoltV3KillSwitchCancelSupervisor::plan_cancel(cancel_plan_request(cancelling_state()))
            .expect("cancelling durable state should allow proof-only cancel planning");

    assert_eq!(
        plan.commands().len(),
        BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces().len()
    );
    let command = plan
        .commands()
        .iter()
        .find(|command| command.client_order_id() == client_order_id("client-order-0"))
        .expect("first candidate command should exist");

    assert_eq!(command.halt_id(), "halt-1");
    assert_eq!(command.action_id(), ACTION_ID);
    assert_eq!(command.config_sha256(), CONFIG_SHA256);
    assert_eq!(command.policy_sha256(), POLICY_SHA256);
    assert_eq!(
        command.source_timestamp_unix_nanos(),
        REQUEST_SOURCE_TIMESTAMP_UNIX_NANOS
    );
    assert_eq!(command.account_id(), account_id());
    assert_eq!(command.instrument_id(), instrument_id());
    assert_eq!(
        command.strategy_id(),
        strategy_id("binary-oracle-edge-taker-001")
    );
    assert_eq!(command.client_order_id(), client_order_id("client-order-0"));
    assert_eq!(command.order_status(), OrderStatus::Accepted);
    assert_eq!(
        command.surface(),
        BoltV3KillSwitchOutstandingOrderRiskSurface::Open
    );
}

#[test]
fn cancel_supervisor_rejects_empty_scope_filters() {
    for (account_ids, instrument_ids, strategy_ids) in [
        (
            Vec::<AccountId>::new(),
            vec![instrument_id()],
            vec![strategy_id("binary-oracle-edge-taker-001")],
        ),
        (
            vec![account_id()],
            Vec::<InstrumentId>::new(),
            vec![strategy_id("binary-oracle-edge-taker-001")],
        ),
        (
            vec![account_id()],
            vec![instrument_id()],
            Vec::<StrategyId>::new(),
        ),
    ] {
        assert_eq!(
            BoltV3KillSwitchCancelScope::new(account_ids, instrument_ids, strategy_ids),
            Err(BoltV3KillSwitchCancelError::InvalidScope)
        );
    }
}

#[test]
fn cancel_supervisor_rejects_snapshot_candidates_outside_scope_filters() {
    for scope in [
        BoltV3KillSwitchCancelScope::new(
            vec![account_id_from("GENERIC-002")],
            vec![instrument_id()],
            vec![strategy_id("binary-oracle-edge-taker-001")],
        ),
        BoltV3KillSwitchCancelScope::new(
            vec![account_id()],
            vec![instrument_id_from("ETH-2026-06-02-UP.GENERIC")],
            vec![strategy_id("binary-oracle-edge-taker-001")],
        ),
        BoltV3KillSwitchCancelScope::new(
            vec![account_id()],
            vec![instrument_id()],
            vec![strategy_id("binary-oracle-edge-taker-002")],
        ),
    ] {
        let mut request = cancel_plan_request(cancelling_state());
        request.scope = scope.expect("out-of-scope filter still has valid NT identifiers");

        assert_eq!(
            BoltV3KillSwitchCancelSupervisor::plan_cancel(request),
            Err(BoltV3KillSwitchCancelError::OutOfScopeCancelCandidate)
        );
    }
}

#[test]
fn cancel_supervisor_requires_supported_route_proof_before_planned_commands() {
    let mut missing_route_request = cancel_plan_request(cancelling_state());
    missing_route_request.route_proof = None;
    let missing_route_error = BoltV3KillSwitchCancelSupervisor::plan_cancel(missing_route_request)
        .expect_err("missing route proof must fail closed");
    assert_eq!(
        missing_route_error,
        BoltV3KillSwitchCancelError::FailedManualInterventionRequired
    );

    let mut unsupported_route_request = cancel_plan_request(cancelling_state());
    unsupported_route_request.route_proof = Some(BoltV3KillSwitchCancelRouteProof::new(
        BoltV3KillSwitchCancelRouteKind::Unsupported,
    ));
    let unsupported_route_error =
        BoltV3KillSwitchCancelSupervisor::plan_cancel(unsupported_route_request)
            .expect_err("unsupported route proof must fail closed");
    assert_eq!(
        unsupported_route_error,
        BoltV3KillSwitchCancelError::FailedManualInterventionRequired
    );

    let plan =
        BoltV3KillSwitchCancelSupervisor::plan_cancel(cancel_plan_request(cancelling_state()))
            .expect("supported route proof should allow proof-only cancel planning");
    assert_eq!(
        plan.commands()[0].route_kind(),
        BoltV3KillSwitchCancelRouteKind::PerStrategyActionPort
    );
}

#[test]
fn cancel_outcome_aggregation_uses_nt_order_status_evidence_without_collapsing_races() {
    let requested = BoltV3KillSwitchCancelAttemptOutcome::cancel_requested(OrderStatus::Accepted)
        .expect("requested outcome should preserve accepted NT status");
    let accepted = BoltV3KillSwitchCancelAttemptOutcome::cancel_accepted(OrderStatus::Accepted)
        .expect("accepted outcome should preserve accepted NT status");
    let rejected = BoltV3KillSwitchCancelAttemptOutcome::cancel_rejected(OrderStatus::Accepted)
        .expect("rejected outcome should preserve accepted NT status");
    let pending = BoltV3KillSwitchCancelAttemptOutcome::pending_cancel(OrderStatus::PendingCancel)
        .expect("pending-cancel outcome should require NT pending-cancel status");
    let expired = BoltV3KillSwitchCancelAttemptOutcome::expired(OrderStatus::Expired)
        .expect("expired outcome should require NT expired status");
    let filled = BoltV3KillSwitchCancelAttemptOutcome::filled_before_cancel(OrderStatus::Filled)
        .expect("filled-before-cancel outcome should require NT filled status");
    let terminal =
        BoltV3KillSwitchCancelAttemptOutcome::terminal_before_cancel(OrderStatus::Canceled)
            .expect("terminal-before-cancel outcome should accept closed NT status");

    assert_ne!(requested.kind(), accepted.kind());
    assert_ne!(accepted.kind(), rejected.kind());
    assert_ne!(pending.kind(), expired.kind());
    assert_eq!(pending.order_status(), OrderStatus::PendingCancel);

    assert_eq!(
        aggregate_single_candidate(pending).result(),
        BoltV3KillSwitchCancelAggregateResult::OutstandingRiskRemains
    );
    assert_eq!(
        aggregate_single_candidate(expired).result(),
        BoltV3KillSwitchCancelAggregateResult::AllTerminal
    );
    assert_eq!(
        aggregate_single_candidate(terminal).result(),
        BoltV3KillSwitchCancelAggregateResult::AllTerminal
    );
    assert_eq!(
        aggregate_single_candidate(filled).result(),
        BoltV3KillSwitchCancelAggregateResult::RequiresPositionReconciliation
    );
    assert_eq!(
        aggregate_single_candidate(rejected).result(),
        BoltV3KillSwitchCancelAggregateResult::FailedManualIntervention
    );
}

#[test]
fn cancel_attempt_outcomes_reject_terminal_nt_status_fail_closed() {
    for terminal_status in [
        OrderStatus::Denied,
        OrderStatus::Rejected,
        OrderStatus::Canceled,
        OrderStatus::Expired,
        OrderStatus::Filled,
    ] {
        assert_eq!(
            BoltV3KillSwitchCancelAttemptOutcome::cancel_requested(terminal_status),
            Err(BoltV3KillSwitchCancelError::InvalidOutcomeOrderStatus),
            "cancel-requested must reject terminal NT status {terminal_status:?}"
        );
        assert_eq!(
            BoltV3KillSwitchCancelAttemptOutcome::cancel_accepted(terminal_status),
            Err(BoltV3KillSwitchCancelError::InvalidOutcomeOrderStatus),
            "cancel-accepted must reject terminal NT status {terminal_status:?}"
        );
        assert_eq!(
            BoltV3KillSwitchCancelAttemptOutcome::cancel_rejected(terminal_status),
            Err(BoltV3KillSwitchCancelError::InvalidOutcomeOrderStatus),
            "cancel-rejected must reject terminal NT status {terminal_status:?}"
        );
    }

    BoltV3KillSwitchCancelAttemptOutcome::cancel_requested(OrderStatus::Accepted)
        .expect("cancel-requested must still accept non-terminal NT status");
    BoltV3KillSwitchCancelAttemptOutcome::cancel_accepted(OrderStatus::Accepted)
        .expect("cancel-accepted must still accept non-terminal NT status");
    BoltV3KillSwitchCancelAttemptOutcome::cancel_rejected(OrderStatus::Accepted)
        .expect("cancel-rejected must still accept non-terminal NT status");
}

#[test]
fn cancel_outcome_aggregation_keeps_missing_or_duplicate_evidence_fail_closed() {
    let snapshot = single_candidate_snapshot(BoltV3KillSwitchOutstandingOrderRiskSurface::Open);

    assert_eq!(
        BoltV3KillSwitchCancelOutcomeAggregation::from_snapshot_outcomes(&snapshot, Vec::new())
            .expect("missing outcome evidence should aggregate as outstanding risk")
            .result(),
        BoltV3KillSwitchCancelAggregateResult::OutstandingRiskRemains
    );

    let requested = BoltV3KillSwitchCancelAttemptOutcome::cancel_requested(OrderStatus::Accepted)
        .expect("requested outcome should construct");
    let rejected = BoltV3KillSwitchCancelAttemptOutcome::cancel_rejected(OrderStatus::Accepted)
        .expect("rejected outcome should construct");
    let requested_evidence =
        BoltV3KillSwitchCancelOutcomeEvidence::from_candidate(&snapshot.candidates()[0], requested);
    let rejected_evidence =
        BoltV3KillSwitchCancelOutcomeEvidence::from_candidate(&snapshot.candidates()[0], rejected);

    assert_eq!(
        BoltV3KillSwitchCancelOutcomeAggregation::from_snapshot_outcomes(
            &snapshot,
            vec![requested_evidence, rejected_evidence],
        )
        .expect("duplicate outcome evidence should preserve worst observed state")
        .result(),
        BoltV3KillSwitchCancelAggregateResult::FailedManualIntervention
    );
}

fn cancel_plan_request(kill_switch_state: KillSwitchState) -> BoltV3KillSwitchCancelPlanRequest {
    BoltV3KillSwitchCancelPlanRequest {
        kill_switch_state,
        action_id: ACTION_ID.to_string(),
        config_sha256: CONFIG_SHA256.to_string(),
        policy_sha256: POLICY_SHA256.to_string(),
        source_timestamp_unix_nanos: REQUEST_SOURCE_TIMESTAMP_UNIX_NANOS,
        observed_at_unix_nanos: REQUEST_OBSERVED_AT_UNIX_NANOS,
        scope: valid_scope(),
        route_proof: Some(BoltV3KillSwitchCancelRouteProof::new(
            BoltV3KillSwitchCancelRouteKind::PerStrategyActionPort,
        )),
        policy: mandatory_surface_policy(),
        snapshot: complete_cancel_snapshot(),
    }
}

fn mandatory_surface_policy() -> BoltV3KillSwitchCancelPolicy {
    BoltV3KillSwitchCancelPolicy::new(
        BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces()
            .iter()
            .copied(),
    )
    .expect("mandatory surface policy should construct")
}

fn complete_cancel_snapshot() -> BoltV3KillSwitchCancelSnapshot {
    complete_cancel_snapshot_with_timestamp(1_717_200_000_000_000_000)
}

fn complete_cancel_snapshot_with_timestamp(
    source_timestamp_unix_nanos: u64,
) -> BoltV3KillSwitchCancelSnapshot {
    let candidates = BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces()
        .iter()
        .enumerate()
        .map(|(index, surface)| {
            cancel_candidate_with_timestamp(
                *surface,
                &format!("client-order-{index}"),
                source_timestamp_unix_nanos,
            )
        })
        .collect::<Vec<_>>();
    BoltV3KillSwitchCancelSnapshot::new(candidates).expect("complete snapshot should construct")
}

fn valid_scope() -> BoltV3KillSwitchCancelScope {
    BoltV3KillSwitchCancelScope::new(
        vec![account_id()],
        vec![instrument_id()],
        vec![strategy_id("binary-oracle-edge-taker-001")],
    )
    .expect("valid cancel scope should construct")
}

fn aggregate_single_candidate(
    outcome: BoltV3KillSwitchCancelAttemptOutcome,
) -> BoltV3KillSwitchCancelOutcomeAggregation {
    let snapshot = single_candidate_snapshot(BoltV3KillSwitchOutstandingOrderRiskSurface::Open);
    let evidence =
        BoltV3KillSwitchCancelOutcomeEvidence::from_candidate(&snapshot.candidates()[0], outcome);
    BoltV3KillSwitchCancelOutcomeAggregation::from_snapshot_outcomes(&snapshot, vec![evidence])
        .expect("single candidate outcome should aggregate")
}

fn single_candidate_snapshot(
    surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
) -> BoltV3KillSwitchCancelSnapshot {
    BoltV3KillSwitchCancelSnapshot::new(vec![cancel_candidate(surface, "client-order-0")])
        .expect("single candidate snapshot should construct")
}

fn non_cancelling_states() -> Vec<KillSwitchState> {
    vec![
        KillSwitchState::Armed,
        KillSwitchState::Halting {
            halt_id: "halt-1".to_string(),
            trigger: loss_trigger(),
        },
        KillSwitchState::Halted {
            halt_id: "halt-1".to_string(),
            trigger: loss_trigger(),
        },
        KillSwitchState::Flattening {
            halt_id: "halt-1".to_string(),
        },
        KillSwitchState::Flat {
            halt_id: "halt-1".to_string(),
        },
        KillSwitchState::FailedManualIntervention {
            halt_id: "halt-1".to_string(),
            reason: "cancel route proof missing".to_string(),
        },
    ]
}

fn cancelling_state() -> KillSwitchState {
    KillSwitchState::Cancelling {
        halt_id: "halt-1".to_string(),
    }
}

fn loss_trigger() -> KillSwitchHaltTrigger {
    KillSwitchHaltTrigger::loss_governor_breach(
        "loss-governor",
        1_717_200_000_000_000_000,
        "daily loss cap breached",
    )
}

fn cancel_candidate(
    surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
    client_order_id: &str,
) -> BoltV3KillSwitchCancelCandidate {
    cancel_candidate_for_strategy(surface, client_order_id, "binary-oracle-edge-taker-001")
}

fn cancel_candidate_for_strategy(
    surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
    client_order_id: &str,
    strategy_id: &str,
) -> BoltV3KillSwitchCancelCandidate {
    cancel_candidate_for_strategy_with_timestamp(
        surface,
        client_order_id,
        strategy_id,
        1_717_200_000_000_000_000,
    )
}

fn cancel_candidate_with_timestamp(
    surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
    client_order_id: &str,
    source_timestamp_unix_nanos: u64,
) -> BoltV3KillSwitchCancelCandidate {
    cancel_candidate_for_strategy_with_timestamp(
        surface,
        client_order_id,
        "binary-oracle-edge-taker-001",
        source_timestamp_unix_nanos,
    )
}

fn cancel_candidate_for_strategy_with_timestamp(
    surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
    client_order_id_value: &str,
    strategy_id_value: &str,
    source_timestamp_unix_nanos: u64,
) -> BoltV3KillSwitchCancelCandidate {
    BoltV3KillSwitchCancelCandidate::from_nt_order_state(
        surface,
        account_id(),
        instrument_id(),
        strategy_id(strategy_id_value),
        client_order_id(client_order_id_value),
        order_status_for_surface(surface),
        source_timestamp_unix_nanos,
    )
    .expect("cancel candidate should be valid")
}

fn order_status_for_surface(surface: BoltV3KillSwitchOutstandingOrderRiskSurface) -> OrderStatus {
    match surface {
        BoltV3KillSwitchOutstandingOrderRiskSurface::Open => OrderStatus::Accepted,
        BoltV3KillSwitchOutstandingOrderRiskSurface::Inflight => OrderStatus::Submitted,
        BoltV3KillSwitchOutstandingOrderRiskSurface::PendingCancel => OrderStatus::PendingCancel,
        BoltV3KillSwitchOutstandingOrderRiskSurface::Emulated => OrderStatus::Emulated,
        BoltV3KillSwitchOutstandingOrderRiskSurface::AlgorithmManaged
        | BoltV3KillSwitchOutstandingOrderRiskSurface::Contingent
        | BoltV3KillSwitchOutstandingOrderRiskSurface::AcceptedButNotTerminal => {
            OrderStatus::Accepted
        }
    }
}

fn account_id() -> AccountId {
    AccountId::new("GENERIC-001")
}

fn account_id_from(value: &str) -> AccountId {
    AccountId::new(value)
}

fn instrument_id() -> InstrumentId {
    instrument_id_from("BTC-2026-06-02-UP.GENERIC")
}

fn instrument_id_from(value: &str) -> InstrumentId {
    InstrumentId::from_as_ref(value).expect("test instrument ID should parse through NT")
}

fn strategy_id(value: &str) -> StrategyId {
    StrategyId::new(value)
}

fn client_order_id(value: &str) -> ClientOrderId {
    ClientOrderId::new(value)
}
