use bolt_v2::bolt_v3_maker_market_selection::{
    MakerMarketCandidate, MakerMarketCandidateRejectReason, MakerMarketPortfolioBlocker,
    MakerMarketPortfolioPolicy, MakerMarketRotationReason, MakerMarketSlotState,
    MakerPerMarketHealth, MakerPerMarketKill, plan_maker_market_portfolio,
};

fn policy(max_active_markets: usize, total_bankroll_notional: f64) -> MakerMarketPortfolioPolicy {
    MakerMarketPortfolioPolicy {
        max_active_markets,
        total_bankroll_notional,
        min_slot_notional: 10.0,
    }
}

fn candidate(
    market_key: &'static str,
    eligible: bool,
    rotation_rank: u64,
) -> MakerMarketCandidate<'static> {
    MakerMarketCandidate {
        market_key,
        eligible,
        rotation_rank,
    }
}

fn active(
    market_key: &'static str,
    health: MakerPerMarketHealth,
    kill: MakerPerMarketKill,
) -> MakerMarketSlotState<'static> {
    MakerMarketSlotState {
        market_key,
        health,
        kill,
    }
}

#[test]
fn portfolio_planner_filters_eligibility_enforces_cap_and_splits_capital() {
    let decision = plan_maker_market_portfolio(
        policy(2, 100.0),
        &[
            candidate("market-a", true, 20),
            candidate("market-b", false, 10),
            candidate("market-c", true, 30),
            candidate("market-d", true, 40),
        ],
        &[],
    );

    assert!(decision.blockers.is_empty());
    let plan = decision.plan.expect("eligible portfolio should plan");
    let slot_keys: Vec<_> = plan.slots.iter().map(|slot| slot.market_key).collect();
    assert_eq!(slot_keys, vec!["market-a", "market-c"]);
    assert!(plan.slots.iter().all(|slot| !slot.retained));
    assert!(
        plan.slots
            .iter()
            .all(|slot| slot.allocation_notional == 50.0)
    );
    assert!(
        plan.rejected_candidates.iter().any(|rejection| {
            rejection.market_key == "market-b"
                && rejection.reason == MakerMarketCandidateRejectReason::Ineligible
        }),
        "ineligible discovered markets must be rejected before selection"
    );
    assert!(
        plan.rejected_candidates.iter().any(|rejection| {
            rejection.market_key == "market-d"
                && rejection.reason == MakerMarketCandidateRejectReason::OverConcurrencyCap
        }),
        "eligible markets above the concurrency cap must be recorded as over-cap"
    );
}

#[test]
fn portfolio_planner_retains_healthy_active_slots_before_filling_new_capacity() {
    let decision = plan_maker_market_portfolio(
        policy(2, 80.0),
        &[
            candidate("market-new-first", true, 1),
            candidate("market-active", true, 99),
            candidate("market-new-second", true, 2),
        ],
        &[active(
            "market-active",
            MakerPerMarketHealth::Healthy,
            MakerPerMarketKill::Clear,
        )],
    );

    let plan = decision.plan.expect("portfolio should retain and fill");
    let retained = plan
        .slots
        .iter()
        .find(|slot| slot.market_key == "market-active")
        .expect("active market should remain selected");
    assert!(retained.retained);
    assert_eq!(plan.slots.len(), 2);
    assert!(
        plan.slots
            .iter()
            .any(|slot| slot.market_key == "market-new-first" && !slot.retained),
        "one free slot should be filled by the best-ranked discovered candidate"
    );
}

#[test]
fn portfolio_planner_rotates_extra_active_slots_above_the_cap() {
    let decision = plan_maker_market_portfolio(
        policy(1, 80.0),
        &[
            candidate("active-a", true, 1),
            candidate("active-b", true, 2),
            candidate("replacement", true, 3),
        ],
        &[
            active(
                "active-a",
                MakerPerMarketHealth::Healthy,
                MakerPerMarketKill::Clear,
            ),
            active(
                "active-b",
                MakerPerMarketHealth::Healthy,
                MakerPerMarketKill::Clear,
            ),
        ],
    );

    let plan = decision.plan.expect("first active slot should be retained");
    assert_eq!(plan.slots.len(), 1);
    assert_eq!(plan.slots[0].market_key, "active-a");
    assert!(plan.slots[0].retained);
    assert!(
        plan.rotated_out.iter().any(|rotation| {
            rotation.market_key == "active-b"
                && rotation.reason == MakerMarketRotationReason::OverConcurrencyCap
        }),
        "extra active markets must rotate out instead of exceeding the cap"
    );
    assert!(
        plan.rejected_candidates.iter().any(|rejection| {
            rejection.market_key == "replacement"
                && rejection.reason == MakerMarketCandidateRejectReason::OverConcurrencyCap
        }),
        "new candidates must not refill beyond the already-retained capped slot"
    );
}

#[test]
fn portfolio_planner_auto_rotates_missing_unhealthy_and_killed_markets() {
    let decision = plan_maker_market_portfolio(
        policy(3, 90.0),
        &[
            candidate("replacement-a", true, 1),
            candidate("replacement-b", true, 2),
            candidate("replacement-c", true, 3),
            candidate("unhealthy-active", true, 4),
            candidate("killed-active", true, 5),
        ],
        &[
            active(
                "missing-active",
                MakerPerMarketHealth::Healthy,
                MakerPerMarketKill::Clear,
            ),
            active(
                "unhealthy-active",
                MakerPerMarketHealth::Blocked,
                MakerPerMarketKill::Clear,
            ),
            active(
                "killed-active",
                MakerPerMarketHealth::Healthy,
                MakerPerMarketKill::Killed,
            ),
        ],
    );

    let plan = decision
        .plan
        .expect("replacements should fill rotated slots");
    assert_eq!(plan.slots.len(), 3);
    let slot_keys: Vec<_> = plan.slots.iter().map(|slot| slot.market_key).collect();
    assert_eq!(
        slot_keys,
        vec!["replacement-a", "replacement-b", "replacement-c"]
    );
    assert!(
        plan.rotated_out.iter().any(|rotation| {
            rotation.market_key == "missing-active"
                && rotation.reason == MakerMarketRotationReason::MissingFromDiscovery
        }),
        "active market absent from discovery must rotate out"
    );
    assert!(
        plan.rotated_out.iter().any(|rotation| {
            rotation.market_key == "unhealthy-active"
                && rotation.reason == MakerMarketRotationReason::PerMarketHealthBlocked
        }),
        "only the unhealthy market should be rotated for its local health"
    );
    assert!(
        plan.rotated_out.iter().any(|rotation| {
            rotation.market_key == "killed-active"
                && rotation.reason == MakerMarketRotationReason::PerMarketKilled
        }),
        "only the killed market should be rotated for its local kill state"
    );
}

#[test]
fn portfolio_planner_fails_closed_for_invalid_policy_and_duplicate_inputs() {
    let decision = plan_maker_market_portfolio(
        MakerMarketPortfolioPolicy {
            max_active_markets: 0,
            total_bankroll_notional: f64::NAN,
            min_slot_notional: 0.0,
        },
        &[
            candidate("", true, 1),
            candidate("duplicate-market", true, 2),
            candidate("duplicate-market", true, 3),
        ],
        &[
            active("", MakerPerMarketHealth::Healthy, MakerPerMarketKill::Clear),
            active(
                "duplicate-active",
                MakerPerMarketHealth::Healthy,
                MakerPerMarketKill::Clear,
            ),
            active(
                "duplicate-active",
                MakerPerMarketHealth::Healthy,
                MakerPerMarketKill::Clear,
            ),
        ],
    );

    assert!(decision.plan.is_none());
    assert!(
        decision
            .blockers
            .contains(&MakerMarketPortfolioBlocker::InvalidMaxActiveMarkets)
    );
    assert!(
        decision
            .blockers
            .contains(&MakerMarketPortfolioBlocker::InvalidTotalBankroll)
    );
    assert!(
        decision
            .blockers
            .contains(&MakerMarketPortfolioBlocker::InvalidMinSlotNotional)
    );
    assert!(
        decision
            .blockers
            .contains(&MakerMarketPortfolioBlocker::EmptyCandidateMarketKey)
    );
    assert!(
        decision
            .blockers
            .contains(&MakerMarketPortfolioBlocker::DuplicateCandidateMarket {
                market_key: "duplicate-market",
            },)
    );
    assert!(
        decision
            .blockers
            .contains(&MakerMarketPortfolioBlocker::EmptyActiveMarketKey)
    );
    assert!(
        decision
            .blockers
            .contains(&MakerMarketPortfolioBlocker::DuplicateActiveMarket {
                market_key: "duplicate-active",
            })
    );
}

#[test]
fn portfolio_planner_blocks_when_no_candidate_is_eligible() {
    let decision = plan_maker_market_portfolio(
        policy(2, 100.0),
        &[
            candidate("market-a", false, 1),
            candidate("market-b", false, 2),
        ],
        &[],
    );

    assert!(decision.plan.is_none());
    assert_eq!(
        decision.blockers,
        vec![MakerMarketPortfolioBlocker::NoEligibleCandidates]
    );
}

#[test]
fn portfolio_planner_blocks_when_split_allocation_is_below_floor() {
    let decision = plan_maker_market_portfolio(
        policy(2, 15.0),
        &[
            candidate("market-a", true, 1),
            candidate("market-b", true, 2),
        ],
        &[],
    );

    assert!(decision.plan.is_none());
    assert_eq!(
        decision.blockers,
        vec![MakerMarketPortfolioBlocker::InsufficientSlotAllocation]
    );
}
