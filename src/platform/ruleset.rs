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

pub fn select_market(ruleset: &RulesetConfig, candidates: &[CandidateMarket]) -> SelectionDecision {
    let mut eligible: Vec<CandidateMarket> = candidates
        .iter()
        .filter(|market| market.tag_slug == ruleset.tag_slug)
        .filter(|market| market.declared_resolution_basis == ruleset.resolution_basis)
        .filter(|market| !ruleset.require_accepting_orders || market.accepting_orders)
        .filter(|market| market.liquidity_num >= ruleset.min_liquidity_num)
        .filter(|market| market.seconds_to_end >= ruleset.min_time_to_expiry_secs)
        .filter(|market| market.seconds_to_end <= ruleset.max_time_to_expiry_secs)
        .cloned()
        .collect();
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
                market,
                reason: "freeze window".to_string(),
            }
        }
        Some(market) => SelectionState::Active { market },
    };

    SelectionDecision {
        ruleset_id: ruleset.id.clone(),
        state,
    }
}
