use bolt_v2::bolt_v3_kill_switch_cancel::{
    BoltV3KillSwitchCancelCandidate, BoltV3KillSwitchCancelError, BoltV3KillSwitchCancelPolicy,
    BoltV3KillSwitchCancelSnapshot, BoltV3KillSwitchOutstandingOrderRiskSurface,
};

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
    BoltV3KillSwitchCancelCandidate::new(
        surface,
        "POLYMARKET-001",
        "BTC-2026-06-02-UP",
        strategy_id,
        client_order_id,
        1_717_200_000_000_000_000,
    )
    .expect("cancel candidate should be valid")
}
