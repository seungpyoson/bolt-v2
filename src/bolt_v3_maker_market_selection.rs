//! Generic maker portfolio market-selection planner.
//!
//! The planner is intentionally pure: discovery, venue snapshots, and family
//! matching happen upstream. This module owns the shared Slice 9 policy once
//! candidates exist: filter eligibility, preserve healthy active slots, rotate
//! out blocked or missing markets, enforce the concurrency cap, and split
//! bankroll across isolated per-market slots.

use std::collections::{BTreeMap, BTreeSet};

use crate::bolt_v3_numeric::{
    MILLIS_PER_SECOND_U64, NANOS_PER_MILLI_U64, UNIT_F64, is_positive_finite,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerRuntimeParameterBounds {
    pub trade_flow_window_secs: u64,
    pub trade_flow_max_samples: u64,
    pub mu_min_classified_samples: u64,
    pub mu_stale_window_ms: u64,
    pub mu_min_floor: f64,
    pub requote_min_interval_ms: u64,
    pub quote_interval_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerRuntimeParameterBoundInputs {
    pub trade_flow_window_secs: Option<u64>,
    pub trade_flow_max_samples: Option<u64>,
    pub mu_min_classified_samples: Option<u64>,
    pub mu_stale_window_ms: Option<u64>,
    pub mu_min_floor: Option<f64>,
    pub requote_min_interval_ms: Option<u64>,
    pub quote_interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MakerRuntimeParameterBlocker {
    ZeroTradeFlowWindowSecs,
    TradeFlowWindowMillisOverflow {
        window_secs: u64,
    },
    ZeroTradeFlowMaxSamples,
    ZeroMuMinClassifiedSamples,
    MuMinClassifiedSamplesAboveMax {
        min_classified_samples: u64,
        max_samples: u64,
    },
    ZeroMuStaleWindowMs,
    MuMinFloorOutOfRange {
        floor: f64,
    },
    ZeroRequoteMinIntervalMs,
    ZeroQuoteIntervalMs,
    QuoteIntervalNanosOverflow {
        quote_interval_ms: u64,
    },
}

#[must_use]
pub fn maker_runtime_parameter_bound_blockers(
    bounds: MakerRuntimeParameterBounds,
) -> Vec<MakerRuntimeParameterBlocker> {
    maker_runtime_parameter_input_blockers(MakerRuntimeParameterBoundInputs {
        trade_flow_window_secs: Some(bounds.trade_flow_window_secs),
        trade_flow_max_samples: Some(bounds.trade_flow_max_samples),
        mu_min_classified_samples: Some(bounds.mu_min_classified_samples),
        mu_stale_window_ms: Some(bounds.mu_stale_window_ms),
        mu_min_floor: Some(bounds.mu_min_floor),
        requote_min_interval_ms: Some(bounds.requote_min_interval_ms),
        quote_interval_ms: Some(bounds.quote_interval_ms),
    })
}

#[must_use]
pub fn maker_runtime_parameter_input_blockers(
    inputs: MakerRuntimeParameterBoundInputs,
) -> Vec<MakerRuntimeParameterBlocker> {
    let mut blockers = Vec::new();
    if inputs.trade_flow_window_secs == Some(0) {
        blockers.push(MakerRuntimeParameterBlocker::ZeroTradeFlowWindowSecs);
    }
    if let Some(window_secs) = inputs.trade_flow_window_secs
        && window_secs.checked_mul(MILLIS_PER_SECOND_U64).is_none()
    {
        blockers.push(MakerRuntimeParameterBlocker::TradeFlowWindowMillisOverflow { window_secs });
    }
    if inputs.trade_flow_max_samples == Some(0) {
        blockers.push(MakerRuntimeParameterBlocker::ZeroTradeFlowMaxSamples);
    }
    if inputs.mu_min_classified_samples == Some(0) {
        blockers.push(MakerRuntimeParameterBlocker::ZeroMuMinClassifiedSamples);
    }
    if let (Some(min_classified_samples), Some(max_samples)) = (
        inputs.mu_min_classified_samples,
        inputs.trade_flow_max_samples,
    ) && min_classified_samples > max_samples
    {
        blockers.push(
            MakerRuntimeParameterBlocker::MuMinClassifiedSamplesAboveMax {
                min_classified_samples,
                max_samples,
            },
        );
    }
    if inputs.mu_stale_window_ms == Some(0) {
        blockers.push(MakerRuntimeParameterBlocker::ZeroMuStaleWindowMs);
    }
    if let Some(floor) = inputs.mu_min_floor
        && (!is_positive_finite(floor) || floor >= UNIT_F64)
    {
        blockers.push(MakerRuntimeParameterBlocker::MuMinFloorOutOfRange { floor });
    }
    if inputs.requote_min_interval_ms == Some(0) {
        blockers.push(MakerRuntimeParameterBlocker::ZeroRequoteMinIntervalMs);
    }
    if inputs.quote_interval_ms == Some(0) {
        blockers.push(MakerRuntimeParameterBlocker::ZeroQuoteIntervalMs);
    }
    if let Some(quote_interval_ms) = inputs.quote_interval_ms
        && quote_interval_ms.checked_mul(NANOS_PER_MILLI_U64).is_none()
    {
        blockers
            .push(MakerRuntimeParameterBlocker::QuoteIntervalNanosOverflow { quote_interval_ms });
    }
    blockers
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerMarketPortfolioPolicy {
    pub max_active_markets: usize,
    pub total_bankroll_notional: f64,
    pub min_slot_notional: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerMarketPortfolioPolicyInputs {
    pub max_active_markets: Option<usize>,
    pub total_bankroll_notional: Option<f64>,
    pub min_slot_notional: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerMarketPortfolioDeclarationInputs {
    pub policy: Option<MakerMarketPortfolioPolicy>,
    pub declared_market_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MakerMarketPortfolioDeclarationBlocker {
    EmptyMarkets,
    MarketsAboveActiveCap {
        declared_market_count: usize,
        max_active_markets: usize,
    },
    BankrollBelowMinSlotFloor {
        fundable_slots: usize,
        total_bankroll_notional: f64,
        min_slot_notional: f64,
    },
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
pub fn maker_market_portfolio_policy_blockers<'a>(
    policy: MakerMarketPortfolioPolicy,
) -> Vec<MakerMarketPortfolioBlocker<'a>> {
    maker_market_portfolio_policy_input_blockers(MakerMarketPortfolioPolicyInputs {
        max_active_markets: Some(policy.max_active_markets),
        total_bankroll_notional: Some(policy.total_bankroll_notional),
        min_slot_notional: Some(policy.min_slot_notional),
    })
}

#[must_use]
pub fn maker_market_portfolio_policy_input_blockers<'a>(
    inputs: MakerMarketPortfolioPolicyInputs,
) -> Vec<MakerMarketPortfolioBlocker<'a>> {
    let mut blockers = Vec::new();
    if inputs.max_active_markets == Some(0) {
        blockers.push(MakerMarketPortfolioBlocker::InvalidMaxActiveMarkets);
    }
    if inputs
        .total_bankroll_notional
        .is_some_and(|value| !is_positive_finite(value))
    {
        blockers.push(MakerMarketPortfolioBlocker::InvalidTotalBankroll);
    }
    if inputs
        .min_slot_notional
        .is_some_and(|value| !is_positive_finite(value))
    {
        blockers.push(MakerMarketPortfolioBlocker::InvalidMinSlotNotional);
    }
    blockers
}

#[must_use]
pub fn maker_market_portfolio_declaration_blockers(
    inputs: MakerMarketPortfolioDeclarationInputs,
) -> Vec<MakerMarketPortfolioDeclarationBlocker> {
    let mut blockers = Vec::new();
    if inputs.declared_market_count == 0 {
        blockers.push(MakerMarketPortfolioDeclarationBlocker::EmptyMarkets);
    }
    let Some(policy) = inputs.policy else {
        return blockers;
    };
    if inputs.declared_market_count > policy.max_active_markets {
        blockers.push(
            MakerMarketPortfolioDeclarationBlocker::MarketsAboveActiveCap {
                declared_market_count: inputs.declared_market_count,
                max_active_markets: policy.max_active_markets,
            },
        );
    }
    let fundable_slots = inputs.declared_market_count.min(policy.max_active_markets);
    if fundable_slots > 0
        && is_positive_finite(policy.total_bankroll_notional)
        && is_positive_finite(policy.min_slot_notional)
        && policy.total_bankroll_notional / (fundable_slots as f64) < policy.min_slot_notional
    {
        blockers.push(
            MakerMarketPortfolioDeclarationBlocker::BankrollBelowMinSlotFloor {
                fundable_slots,
                total_bankroll_notional: policy.total_bankroll_notional,
                min_slot_notional: policy.min_slot_notional,
            },
        );
    }
    blockers
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
    let mut blockers = maker_market_portfolio_policy_blockers(policy);
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
