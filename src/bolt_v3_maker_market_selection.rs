//! Generic maker portfolio market-selection planner.
//!
//! The planner is intentionally pure: discovery, venue snapshots, and family
//! matching happen upstream. This module owns the shared Slice 9 policy once
//! candidates exist: filter eligibility, preserve healthy active slots, rotate
//! out blocked or missing markets, enforce the concurrency cap, and split
//! bankroll across isolated per-market slots.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerMarketPortfolioPolicy {
    pub max_active_markets: usize,
    pub total_bankroll_notional: f64,
    pub min_slot_notional: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerMarketCandidate<'a> {
    pub market_key: &'a str,
    pub eligible: bool,
    pub rotation_rank: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerPerMarketHealth {
    Healthy,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerPerMarketKill {
    Clear,
    Killed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerMarketSlotState<'a> {
    pub market_key: &'a str,
    pub health: MakerPerMarketHealth,
    pub kill: MakerPerMarketKill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerMarketPortfolioBlocker<'a> {
    InvalidMaxActiveMarkets,
    InvalidTotalBankroll,
    InvalidMinSlotNotional,
    EmptyCandidateMarketKey,
    DuplicateCandidateMarket { market_key: &'a str },
    EmptyActiveMarketKey,
    DuplicateActiveMarket { market_key: &'a str },
    NoEligibleCandidates,
    InsufficientSlotAllocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerMarketCandidateRejectReason {
    Ineligible,
    OverConcurrencyCap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerMarketCandidateRejection<'a> {
    pub market_key: &'a str,
    pub reason: MakerMarketCandidateRejectReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerMarketRotationReason {
    MissingFromDiscovery,
    CandidateIneligible,
    PerMarketHealthBlocked,
    PerMarketKilled,
    OverConcurrencyCap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerMarketRotation<'a> {
    pub market_key: &'a str,
    pub reason: MakerMarketRotationReason,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerMarketSlotPlan<'a> {
    pub market_key: &'a str,
    pub allocation_notional: f64,
    pub retained: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerMarketPortfolioPlan<'a> {
    pub slots: Vec<MakerMarketSlotPlan<'a>>,
    pub rejected_candidates: Vec<MakerMarketCandidateRejection<'a>>,
    pub rotated_out: Vec<MakerMarketRotation<'a>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerMarketPortfolioDecision<'a> {
    pub plan: Option<MakerMarketPortfolioPlan<'a>>,
    pub blockers: Vec<MakerMarketPortfolioBlocker<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MakerMarketSlotSeed<'a> {
    market_key: &'a str,
    retained: bool,
}

#[must_use]
pub fn plan_maker_market_portfolio<'a>(
    policy: MakerMarketPortfolioPolicy,
    candidates: &[MakerMarketCandidate<'a>],
    active_slots: &[MakerMarketSlotState<'a>],
) -> MakerMarketPortfolioDecision<'a> {
    let blockers = portfolio_input_blockers(policy, candidates, active_slots);
    if !blockers.is_empty() {
        return MakerMarketPortfolioDecision {
            plan: None,
            blockers,
        };
    }

    let candidates_by_key: BTreeMap<&'a str, MakerMarketCandidate<'a>> = candidates
        .iter()
        .map(|candidate| (candidate.market_key, *candidate))
        .collect();
    let mut selected_keys = BTreeSet::new();
    let mut selected_slots = Vec::new();
    let mut rotated_out = Vec::new();
    let mut rejected_candidates = Vec::new();

    for active in active_slots {
        let Some(candidate) = candidates_by_key.get(active.market_key).copied() else {
            rotated_out.push(MakerMarketRotation {
                market_key: active.market_key,
                reason: MakerMarketRotationReason::MissingFromDiscovery,
            });
            continue;
        };
        if !candidate.eligible {
            rotated_out.push(MakerMarketRotation {
                market_key: active.market_key,
                reason: MakerMarketRotationReason::CandidateIneligible,
            });
            rejected_candidates.push(MakerMarketCandidateRejection {
                market_key: active.market_key,
                reason: MakerMarketCandidateRejectReason::Ineligible,
            });
            continue;
        }
        if active.health != MakerPerMarketHealth::Healthy {
            rotated_out.push(MakerMarketRotation {
                market_key: active.market_key,
                reason: MakerMarketRotationReason::PerMarketHealthBlocked,
            });
            continue;
        }
        if active.kill != MakerPerMarketKill::Clear {
            rotated_out.push(MakerMarketRotation {
                market_key: active.market_key,
                reason: MakerMarketRotationReason::PerMarketKilled,
            });
            continue;
        }
        if selected_slots.len() >= policy.max_active_markets {
            rotated_out.push(MakerMarketRotation {
                market_key: active.market_key,
                reason: MakerMarketRotationReason::OverConcurrencyCap,
            });
            continue;
        }
        selected_keys.insert(active.market_key);
        selected_slots.push(MakerMarketSlotSeed {
            market_key: active.market_key,
            retained: true,
        });
    }

    let mut fill_candidates: Vec<MakerMarketCandidate<'a>> = candidates
        .iter()
        .copied()
        .filter(|candidate| !selected_keys.contains(candidate.market_key))
        .collect();
    fill_candidates.sort_by_key(|candidate| (candidate.rotation_rank, candidate.market_key));

    for candidate in fill_candidates {
        if !candidate.eligible {
            if !rejected_candidates
                .iter()
                .any(|rejection| rejection.market_key == candidate.market_key)
            {
                rejected_candidates.push(MakerMarketCandidateRejection {
                    market_key: candidate.market_key,
                    reason: MakerMarketCandidateRejectReason::Ineligible,
                });
            }
            continue;
        }
        if selected_slots.len() >= policy.max_active_markets {
            rejected_candidates.push(MakerMarketCandidateRejection {
                market_key: candidate.market_key,
                reason: MakerMarketCandidateRejectReason::OverConcurrencyCap,
            });
            continue;
        }
        selected_keys.insert(candidate.market_key);
        selected_slots.push(MakerMarketSlotSeed {
            market_key: candidate.market_key,
            retained: false,
        });
    }

    if selected_slots.is_empty() {
        return MakerMarketPortfolioDecision {
            plan: None,
            blockers: vec![MakerMarketPortfolioBlocker::NoEligibleCandidates],
        };
    }

    let allocation = policy.total_bankroll_notional / selected_slots.len() as f64;
    if allocation < policy.min_slot_notional {
        return MakerMarketPortfolioDecision {
            plan: None,
            blockers: vec![MakerMarketPortfolioBlocker::InsufficientSlotAllocation],
        };
    }
    let slots = selected_slots
        .into_iter()
        .map(|slot| MakerMarketSlotPlan {
            market_key: slot.market_key,
            allocation_notional: allocation,
            retained: slot.retained,
        })
        .collect();

    MakerMarketPortfolioDecision {
        plan: Some(MakerMarketPortfolioPlan {
            slots,
            rejected_candidates,
            rotated_out,
        }),
        blockers: Vec::new(),
    }
}

fn portfolio_input_blockers<'a>(
    policy: MakerMarketPortfolioPolicy,
    candidates: &[MakerMarketCandidate<'a>],
    active_slots: &[MakerMarketSlotState<'a>],
) -> Vec<MakerMarketPortfolioBlocker<'a>> {
    let mut blockers = Vec::new();
    if policy.max_active_markets == 0 {
        blockers.push(MakerMarketPortfolioBlocker::InvalidMaxActiveMarkets);
    }
    if !is_positive_finite(policy.total_bankroll_notional) {
        blockers.push(MakerMarketPortfolioBlocker::InvalidTotalBankroll);
    }
    if !is_positive_finite(policy.min_slot_notional) {
        blockers.push(MakerMarketPortfolioBlocker::InvalidMinSlotNotional);
    }
    push_market_key_blockers(
        candidates.iter().map(|candidate| candidate.market_key),
        MakerMarketPortfolioBlocker::EmptyCandidateMarketKey,
        |market_key| MakerMarketPortfolioBlocker::DuplicateCandidateMarket { market_key },
        &mut blockers,
    );
    push_market_key_blockers(
        active_slots.iter().map(|slot| slot.market_key),
        MakerMarketPortfolioBlocker::EmptyActiveMarketKey,
        |market_key| MakerMarketPortfolioBlocker::DuplicateActiveMarket { market_key },
        &mut blockers,
    );
    blockers
}

fn push_market_key_blockers<'a>(
    keys: impl Iterator<Item = &'a str>,
    empty_blocker: MakerMarketPortfolioBlocker<'a>,
    duplicate_blocker: impl Fn(&'a str) -> MakerMarketPortfolioBlocker<'a>,
    blockers: &mut Vec<MakerMarketPortfolioBlocker<'a>>,
) {
    let mut seen = BTreeSet::new();
    for key in keys {
        if key.is_empty() {
            blockers.push(empty_blocker);
            continue;
        }
        if !seen.insert(key) {
            blockers.push(duplicate_blocker(key));
        }
    }
}

fn is_positive_finite(value: f64) -> bool {
    value.is_finite() && value > 0.0
}
