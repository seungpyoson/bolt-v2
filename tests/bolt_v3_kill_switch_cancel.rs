use bolt_v2::{
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState},
    bolt_v3_kill_switch_cancel::{
        BoltV3KillSwitchCancelCandidate, BoltV3KillSwitchCancelDecisionMode,
        BoltV3KillSwitchCancelError, BoltV3KillSwitchCancelPlanRequest,
        BoltV3KillSwitchCancelPolicy, BoltV3KillSwitchCancelRouteKind,
        BoltV3KillSwitchCancelRouteProof, BoltV3KillSwitchCancelScope,
        BoltV3KillSwitchCancelSnapshot, BoltV3KillSwitchCancelSupervisor,
        BoltV3KillSwitchOutstandingOrderRiskSurface,
    },
};

const ACTION_ID: &str = "cancel-action-1";
const POLICY_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONFIG_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const REQUEST_SOURCE_TIMESTAMP_UNIX_NANOS: u64 = 1_717_200_000_000_000_001;
const REQUEST_OBSERVED_AT_UNIX_NANOS: u64 = 1_717_200_000_000_000_002;
const MAX_SOURCE_AGE_UNIX_NANOS: u64 = 10;

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
fn cancel_candidate_stores_trimmed_scope_and_identity_fields() {
    let candidate = BoltV3KillSwitchCancelCandidate::new(
        BoltV3KillSwitchOutstandingOrderRiskSurface::Open,
        " POLYMARKET-001 ",
        " BTC-2026-06-02-UP ",
        " binary-oracle-edge-taker-001 ",
        " client-order-1 ",
        1_717_200_000_000_000_000,
    )
    .expect("trimmed cancel candidate should be valid");

    assert_eq!(candidate.account_id(), "POLYMARKET-001");
    assert_eq!(candidate.instrument_id(), "BTC-2026-06-02-UP");
    assert_eq!(candidate.strategy_id(), "binary-oracle-edge-taker-001");
    assert_eq!(candidate.client_order_id(), "client-order-1");
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
        .find(|command| command.client_order_id() == "client-order-0")
        .expect("first candidate command should exist");

    assert_eq!(command.halt_id(), "halt-1");
    assert_eq!(command.action_id(), ACTION_ID);
    assert_eq!(command.config_sha256(), CONFIG_SHA256);
    assert_eq!(command.policy_sha256(), POLICY_SHA256);
    assert_eq!(
        command.source_timestamp_unix_nanos(),
        REQUEST_SOURCE_TIMESTAMP_UNIX_NANOS
    );
    assert_eq!(command.account_id(), "POLYMARKET-001");
    assert_eq!(command.instrument_id(), "BTC-2026-06-02-UP");
    assert_eq!(command.strategy_id(), "binary-oracle-edge-taker-001");
    assert_eq!(command.client_order_id(), "client-order-0");
    assert_eq!(
        command.surface(),
        BoltV3KillSwitchOutstandingOrderRiskSurface::Open
    );
}

#[test]
fn cancel_supervisor_rejects_stale_source_timestamps_and_empty_scope_filters() {
    let stale_timestamp = 1_000;
    let stale_request = BoltV3KillSwitchCancelPlanRequest {
        kill_switch_state: cancelling_state(),
        action_id: ACTION_ID.to_string(),
        config_sha256: CONFIG_SHA256.to_string(),
        policy_sha256: POLICY_SHA256.to_string(),
        source_timestamp_unix_nanos: stale_timestamp,
        observed_at_unix_nanos: stale_timestamp + MAX_SOURCE_AGE_UNIX_NANOS + 1,
        scope: valid_scope(),
        route_proof: Some(BoltV3KillSwitchCancelRouteProof::new(
            BoltV3KillSwitchCancelRouteKind::PerStrategyActionPort,
        )),
        policy: mandatory_surface_policy(),
        snapshot: complete_cancel_snapshot_with_timestamp(stale_timestamp),
    };

    let error = BoltV3KillSwitchCancelSupervisor::plan_cancel(stale_request)
        .expect_err("stale request or candidate source timestamps must fail closed");
    assert_eq!(error, BoltV3KillSwitchCancelError::StaleSourceTimestamp);

    for (account_ids, instrument_ids, strategy_ids) in [
        (
            Vec::<String>::new(),
            vec!["BTC-2026-06-02-UP".to_string()],
            vec!["binary-oracle-edge-taker-001".to_string()],
        ),
        (
            vec!["POLYMARKET-001".to_string()],
            Vec::<String>::new(),
            vec!["binary-oracle-edge-taker-001".to_string()],
        ),
        (
            vec!["POLYMARKET-001".to_string()],
            vec!["BTC-2026-06-02-UP".to_string()],
            Vec::<String>::new(),
        ),
    ] {
        assert_eq!(
            BoltV3KillSwitchCancelScope::new(account_ids, instrument_ids, strategy_ids),
            Err(BoltV3KillSwitchCancelError::InvalidScope)
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
    BoltV3KillSwitchCancelPolicy::with_source_freshness(
        BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces()
            .iter()
            .copied(),
        MAX_SOURCE_AGE_UNIX_NANOS,
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
        vec!["POLYMARKET-001".to_string()],
        vec!["BTC-2026-06-02-UP".to_string()],
        vec!["binary-oracle-edge-taker-001".to_string()],
    )
    .expect("valid cancel scope should construct")
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
    client_order_id: &str,
    strategy_id: &str,
    source_timestamp_unix_nanos: u64,
) -> BoltV3KillSwitchCancelCandidate {
    BoltV3KillSwitchCancelCandidate::new(
        surface,
        "POLYMARKET-001",
        "BTC-2026-06-02-UP",
        strategy_id,
        client_order_id,
        source_timestamp_unix_nanos,
    )
    .expect("cancel candidate should be valid")
}
