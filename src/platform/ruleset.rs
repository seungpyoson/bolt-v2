use crate::{
    config::RulesetConfig,
    platform::resolution_basis::{ResolutionBasis, parse_ruleset_resolution_basis},
};
use std::cmp::Ordering;

pub const SELECTION_FREEZE_WINDOW_REASON: &str = "freeze window";

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateMarket {
    pub market_id: String,
    pub market_slug: String,
    pub question_id: String,
    pub instrument_id: String,
    pub condition_id: String,
    pub up_token_id: String,
    pub down_token_id: String,
    pub selected_market_observed_ts_ms: u64,
    pub price_to_beat: Option<f64>,
    pub price_to_beat_source: Option<String>,
    pub price_to_beat_observed_ts_ms: Option<u64>,
    pub start_ts_ms: u64,
    pub end_ts_ms: u64,
    pub declared_resolution_basis: ResolutionBasis,
    pub accepting_orders: bool,
    pub liquidity_num: f64,
    pub seconds_to_end: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectionState {
    Active {
        market: CandidateMarket,
    },
    Freeze {
        market: CandidateMarket,
        reason: String,
    },
    Idle {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectionDecision {
    pub ruleset_id: String,
    pub state: SelectionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EligibilityRejectReason {
    ResolutionBasisMismatch,
    OrdersClosed,
    LowLiquidity,
    ExpiryTooSoon,
    ExpiryTooLate,
}

impl EligibilityRejectReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ResolutionBasisMismatch => "resolution_basis_mismatch",
            Self::OrdersClosed => "orders_closed",
            Self::LowLiquidity => "low_liquidity",
            Self::ExpiryTooSoon => "expiry_too_soon",
            Self::ExpiryTooLate => "expiry_too_late",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RejectedCandidate {
    pub market: CandidateMarket,
    pub reason: EligibilityRejectReason,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectionEvaluation {
    pub decision: SelectionDecision,
    pub eligible_candidates: Vec<CandidateMarket>,
    /// Rejected candidates preserve the original candidate iteration order.
    pub rejected_candidates: Vec<RejectedCandidate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeSelectionSnapshot {
    pub ruleset_id: String,
    pub decision: SelectionDecision,
    pub eligible_candidates: Vec<CandidateMarket>,
    pub published_at_ms: u64,
}

pub fn evaluate_market_selection(
    ruleset: &RulesetConfig,
    candidates: &[CandidateMarket],
) -> SelectionEvaluation {
    let ruleset_basis = parse_ruleset_resolution_basis(&ruleset.resolution_basis)
        .expect("ruleset resolution basis validated at config load");
    let mut eligible: Vec<&CandidateMarket> = Vec::new();
    let mut rejected_candidates = Vec::new();

    for market in candidates {
        match reject_reason(&ruleset_basis, ruleset, market) {
            Some(reason) => rejected_candidates.push(RejectedCandidate {
                market: market.clone(),
                reason,
            }),
            None => eligible.push(market),
        }
    }

    eligible.sort_by(|lhs, rhs| {
        rhs.liquidity_num
            .total_cmp(&lhs.liquidity_num)
            .then_with(|| lhs.market_id.cmp(&rhs.market_id))
    });

    let eligible_candidates: Vec<CandidateMarket> = eligible.into_iter().cloned().collect();

    let state = match eligible_candidates.first() {
        None => SelectionState::Idle {
            reason: "no_selected_market".to_string(),
        },
        Some(market) if market.seconds_to_end <= ruleset.freeze_before_end_secs => {
            SelectionState::Freeze {
                market: market.clone(),
                reason: SELECTION_FREEZE_WINDOW_REASON.to_string(),
            }
        }
        Some(market) => SelectionState::Active {
            market: market.clone(),
        },
    };

    SelectionEvaluation {
        decision: SelectionDecision {
            ruleset_id: ruleset.id.clone(),
            state,
        },
        eligible_candidates,
        rejected_candidates,
    }
}

pub fn select_market(ruleset: &RulesetConfig, candidates: &[CandidateMarket]) -> SelectionDecision {
    evaluate_market_selection(ruleset, candidates).decision
}

fn reject_reason(
    ruleset_basis: &ResolutionBasis,
    ruleset: &RulesetConfig,
    market: &CandidateMarket,
) -> Option<EligibilityRejectReason> {
    if market.declared_resolution_basis != *ruleset_basis {
        return Some(EligibilityRejectReason::ResolutionBasisMismatch);
    }
    if ruleset.require_accepting_orders && !market.accepting_orders {
        return Some(EligibilityRejectReason::OrdersClosed);
    }
    match market.liquidity_num.partial_cmp(&ruleset.min_liquidity_num) {
        Some(Ordering::Equal | Ordering::Greater) => {}
        Some(Ordering::Less) | None => return Some(EligibilityRejectReason::LowLiquidity),
    }
    if market.seconds_to_end < ruleset.min_time_to_expiry_secs {
        return Some(EligibilityRejectReason::ExpiryTooSoon);
    }
    if market.seconds_to_end > ruleset.max_time_to_expiry_secs {
        return Some(EligibilityRejectReason::ExpiryTooLate);
    }
    None
}
