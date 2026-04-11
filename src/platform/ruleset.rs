use crate::config::RulesetConfig;

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateMarket {
    pub market_id: String,
    pub instrument_id: String,
    pub tag_slug: String,
    pub declared_resolution_basis: String,
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
    TagMismatch,
    ResolutionBasisMismatch,
    OrdersClosed,
    LowLiquidity,
    ExpiryTooSoon,
    ExpiryTooLate,
}

impl EligibilityRejectReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TagMismatch => "tag_mismatch",
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
    /// Rejected candidates preserve the original candidate iteration order.
    pub rejected_candidates: Vec<RejectedCandidate>,
}

pub fn evaluate_market_selection(
    ruleset: &RulesetConfig,
    candidates: &[CandidateMarket],
) -> SelectionEvaluation {
    let mut eligible: Vec<&CandidateMarket> = Vec::new();
    let mut rejected_candidates = Vec::new();

    for market in candidates {
        match reject_reason(ruleset, market) {
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

    let state = match eligible.into_iter().next() {
        None => SelectionState::Idle {
            reason: "no eligible market".to_string(),
        },
        Some(market) if market.seconds_to_end <= ruleset.freeze_before_end_secs => {
            SelectionState::Freeze {
                market: market.clone(),
                reason: "freeze window".to_string(),
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
        rejected_candidates,
    }
}

pub fn select_market(ruleset: &RulesetConfig, candidates: &[CandidateMarket]) -> SelectionDecision {
    evaluate_market_selection(ruleset, candidates).decision
}

fn reject_reason(
    ruleset: &RulesetConfig,
    market: &CandidateMarket,
) -> Option<EligibilityRejectReason> {
    if market.tag_slug != ruleset.tag_slug {
        return Some(EligibilityRejectReason::TagMismatch);
    }
    if market.declared_resolution_basis != ruleset.resolution_basis {
        return Some(EligibilityRejectReason::ResolutionBasisMismatch);
    }
    if ruleset.require_accepting_orders && !market.accepting_orders {
        return Some(EligibilityRejectReason::OrdersClosed);
    }
    if market.liquidity_num < ruleset.min_liquidity_num {
        return Some(EligibilityRejectReason::LowLiquidity);
    }
    if market.seconds_to_end < ruleset.min_time_to_expiry_secs {
        return Some(EligibilityRejectReason::ExpiryTooSoon);
    }
    if market.seconds_to_end > ruleset.max_time_to_expiry_secs {
        return Some(EligibilityRejectReason::ExpiryTooLate);
    }
    None
}
