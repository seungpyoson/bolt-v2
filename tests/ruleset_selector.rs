use bolt_v2::{
    config::{RulesetConfig, RulesetVenueKind},
    platform::ruleset::{
        CandidateMarket, EligibilityRejectReason, RejectedCandidate, SelectionDecision,
        SelectionState, evaluate_market_selection, select_market,
    },
};

fn ruleset() -> RulesetConfig {
    RulesetConfig {
        id: "btc-5m".to_string(),
        venue: RulesetVenueKind::Polymarket,
        tag_slug: "bitcoin".to_string(),
        resolution_basis: "binance_btcusdt_1m".to_string(),
        min_time_to_expiry_secs: 120,
        max_time_to_expiry_secs: 1_800,
        min_liquidity_num: 1_000.0,
        require_accepting_orders: true,
        freeze_before_end_secs: 300,
        selector_poll_interval_ms: 1_000,
        candidate_load_timeout_secs: 30,
    }
}

fn candidate(
    market_id: &str,
    declared_resolution_basis: &str,
    liquidity_num: f64,
    seconds_to_end: u64,
) -> CandidateMarket {
    CandidateMarket {
        market_id: market_id.to_string(),
        instrument_id: format!("{market_id}-yes"),
        tag_slug: "bitcoin".to_string(),
        declared_resolution_basis: declared_resolution_basis.to_string(),
        accepting_orders: true,
        liquidity_num,
        seconds_to_end,
    }
}

#[test]
fn rejects_market_when_resolution_basis_mismatches() {
    let ruleset = ruleset();
    let candidates = vec![candidate(
        "market-bad-basis",
        "chainlink_btcusd",
        5_000.0,
        900,
    )];

    let decision = select_market(&ruleset, &candidates);

    assert_eq!(
        decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Idle {
                reason: "no eligible market".to_string(),
            },
        }
    );
}

#[test]
fn exposes_tag_mismatch_rejection() {
    let ruleset = ruleset();
    let candidates = vec![CandidateMarket {
        market_id: "market-wrong-tag".to_string(),
        instrument_id: "market-wrong-tag-yes".to_string(),
        tag_slug: "ethereum".to_string(),
        declared_resolution_basis: "binance_btcusdt_1m".to_string(),
        accepting_orders: true,
        liquidity_num: 9_000.0,
        seconds_to_end: 1_200,
    }];

    let evaluation = evaluate_market_selection(&ruleset, &candidates);

    assert_eq!(
        evaluation.rejected_candidates,
        vec![RejectedCandidate {
            market: CandidateMarket {
                market_id: "market-wrong-tag".to_string(),
                instrument_id: "market-wrong-tag-yes".to_string(),
                tag_slug: "ethereum".to_string(),
                declared_resolution_basis: "binance_btcusdt_1m".to_string(),
                accepting_orders: true,
                liquidity_num: 9_000.0,
                seconds_to_end: 1_200,
            },
            reason: EligibilityRejectReason::TagMismatch,
        }]
    );
}

#[test]
fn selects_best_eligible_market_within_ruleset_window() {
    let ruleset = ruleset();
    let candidates = vec![
        candidate("market-lower-liq", "binance_btcusdt_1m", 2_500.0, 900),
        candidate("market-best", "binance_btcusdt_1m", 7_500.0, 1_200),
        CandidateMarket {
            market_id: "market-wrong-tag".to_string(),
            instrument_id: "market-wrong-tag-yes".to_string(),
            tag_slug: "ethereum".to_string(),
            declared_resolution_basis: "binance_btcusdt_1m".to_string(),
            accepting_orders: true,
            liquidity_num: 9_000.0,
            seconds_to_end: 1_200,
        },
    ];

    let decision = select_market(&ruleset, &candidates);

    assert_eq!(
        decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Active {
                market: candidate("market-best", "binance_btcusdt_1m", 7_500.0, 1_200),
            },
        }
    );
}

#[test]
fn selects_deterministic_winner_when_liquidity_ties() {
    let ruleset = ruleset();
    let candidates = vec![
        candidate("market-b", "binance_btcusdt_1m", 7_500.0, 1_200),
        candidate("market-a", "binance_btcusdt_1m", 7_500.0, 1_200),
    ];

    let decision = select_market(&ruleset, &candidates);

    assert_eq!(
        decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Active {
                market: candidate("market-a", "binance_btcusdt_1m", 7_500.0, 1_200),
            },
        }
    );
}

#[test]
fn evaluate_market_selection_yields_empty_rejected_when_all_candidates_eligible() {
    let ruleset = ruleset();
    let candidates = vec![
        candidate("market-a", "binance_btcusdt_1m", 5_000.0, 600),
        candidate("market-b", "binance_btcusdt_1m", 9_000.0, 1_200),
    ];

    let evaluation = evaluate_market_selection(&ruleset, &candidates);

    assert_eq!(
        evaluation.decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Active {
                market: candidate("market-b", "binance_btcusdt_1m", 9_000.0, 1_200),
            },
        }
    );
    assert!(evaluation.rejected_candidates.is_empty());
}

#[test]
fn uses_first_matching_reject_reason_for_multi_failure_candidate() {
    let ruleset = ruleset();
    let candidates = vec![CandidateMarket {
        market_id: "market-many-failures".to_string(),
        instrument_id: "market-many-failures-yes".to_string(),
        tag_slug: "bitcoin".to_string(),
        declared_resolution_basis: "chainlink_btcusd".to_string(),
        accepting_orders: false,
        liquidity_num: 500.0,
        seconds_to_end: 60,
    }];

    let evaluation = evaluate_market_selection(&ruleset, &candidates);

    assert_eq!(
        evaluation.rejected_candidates,
        vec![RejectedCandidate {
            market: CandidateMarket {
                market_id: "market-many-failures".to_string(),
                instrument_id: "market-many-failures-yes".to_string(),
                tag_slug: "bitcoin".to_string(),
                declared_resolution_basis: "chainlink_btcusd".to_string(),
                accepting_orders: false,
                liquidity_num: 500.0,
                seconds_to_end: 60,
            },
            reason: EligibilityRejectReason::ResolutionBasisMismatch,
        }]
    );
    assert_eq!(
        evaluation.decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Idle {
                reason: "no eligible market".to_string(),
            },
        }
    );
}

#[test]
fn returns_idle_when_no_market_is_eligible() {
    let ruleset = ruleset();
    let candidates = vec![
        CandidateMarket {
            market_id: "market-orders-closed".to_string(),
            instrument_id: "market-orders-closed-yes".to_string(),
            tag_slug: "bitcoin".to_string(),
            declared_resolution_basis: "binance_btcusdt_1m".to_string(),
            accepting_orders: false,
            liquidity_num: 5_000.0,
            seconds_to_end: 600,
        },
        candidate("market-low-liquidity", "binance_btcusdt_1m", 500.0, 600),
        candidate("market-too-soon", "binance_btcusdt_1m", 5_000.0, 60),
        candidate("market-too-late", "binance_btcusdt_1m", 5_000.0, 4_000),
    ];

    let decision = select_market(&ruleset, &candidates);

    assert_eq!(
        decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Idle {
                reason: "no eligible market".to_string(),
            },
        }
    );
}

#[test]
fn rejects_nan_liquidity_candidate_from_selection() {
    let ruleset = ruleset();
    let candidates = vec![candidate(
        "market-nan-liquidity",
        "binance_btcusdt_1m",
        f64::NAN,
        900,
    )];

    let evaluation = evaluate_market_selection(&ruleset, &candidates);

    assert_eq!(
        evaluation.decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Idle {
                reason: "no eligible market".to_string(),
            },
        }
    );
    assert_eq!(evaluation.rejected_candidates.len(), 1);
    let rejected = &evaluation.rejected_candidates[0];
    assert_eq!(rejected.reason, EligibilityRejectReason::LowLiquidity);
    assert_eq!(rejected.market.market_id, "market-nan-liquidity");
    assert_eq!(rejected.market.instrument_id, "market-nan-liquidity-yes");
    assert_eq!(rejected.market.tag_slug, "bitcoin");
    assert_eq!(
        rejected.market.declared_resolution_basis,
        "binance_btcusdt_1m"
    );
    assert!(rejected.market.accepting_orders);
    assert!(rejected.market.liquidity_num.is_nan());
    assert_eq!(rejected.market.seconds_to_end, 900);
}

#[test]
fn selects_valid_market_when_nan_liquidity_candidate_is_present() {
    let ruleset = ruleset();
    let candidates = vec![
        candidate("market-valid", "binance_btcusdt_1m", 7_500.0, 900),
        candidate(
            "market-nan-liquidity",
            "binance_btcusdt_1m",
            f64::NAN,
            1_200,
        ),
    ];

    let evaluation = evaluate_market_selection(&ruleset, &candidates);

    assert_eq!(
        evaluation.decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Active {
                market: candidate("market-valid", "binance_btcusdt_1m", 7_500.0, 900),
            },
        }
    );
    assert_eq!(evaluation.rejected_candidates.len(), 1);
    let rejected = &evaluation.rejected_candidates[0];
    assert_eq!(rejected.reason, EligibilityRejectReason::LowLiquidity);
    assert_eq!(rejected.market.market_id, "market-nan-liquidity");
    assert_eq!(rejected.market.instrument_id, "market-nan-liquidity-yes");
    assert_eq!(rejected.market.tag_slug, "bitcoin");
    assert_eq!(
        rejected.market.declared_resolution_basis,
        "binance_btcusdt_1m"
    );
    assert!(rejected.market.accepting_orders);
    assert!(rejected.market.liquidity_num.is_nan());
    assert_eq!(rejected.market.seconds_to_end, 1_200);
}

#[test]
fn enters_freeze_state_with_rejected_candidates_present() {
    let ruleset = ruleset();
    let candidates = vec![
        candidate("market-rejected", "chainlink_btcusd", 9_500.0, 900),
        candidate("market-freeze", "binance_btcusdt_1m", 9_000.0, 250),
    ];

    let evaluation = evaluate_market_selection(&ruleset, &candidates);

    assert_eq!(
        evaluation.decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Freeze {
                market: candidate("market-freeze", "binance_btcusdt_1m", 9_000.0, 250),
                reason: "freeze window".to_string(),
            },
        }
    );
    assert_eq!(
        evaluation.rejected_candidates,
        vec![RejectedCandidate {
            market: candidate("market-rejected", "chainlink_btcusd", 9_500.0, 900),
            reason: EligibilityRejectReason::ResolutionBasisMismatch,
        }]
    );
}

#[test]
fn enters_freeze_state_near_market_end() {
    let ruleset = ruleset();
    let candidates = vec![
        candidate("market-active", "binance_btcusdt_1m", 4_000.0, 1_000),
        candidate("market-freeze", "binance_btcusdt_1m", 9_000.0, 250),
    ];

    let decision = select_market(&ruleset, &candidates);

    assert_eq!(
        decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Freeze {
                market: candidate("market-freeze", "binance_btcusdt_1m", 9_000.0, 250),
                reason: "freeze window".to_string(),
            },
        }
    );
}

#[test]
fn enters_freeze_state_at_exact_freeze_boundary() {
    let ruleset = ruleset();
    let candidates = vec![candidate(
        "market-freeze-boundary",
        "binance_btcusdt_1m",
        9_000.0,
        300,
    )];

    let decision = select_market(&ruleset, &candidates);

    assert_eq!(
        decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Freeze {
                market: candidate("market-freeze-boundary", "binance_btcusdt_1m", 9_000.0, 300),
                reason: "freeze window".to_string(),
            },
        }
    );
}

#[test]
fn exposes_rejected_candidates_with_explicit_eligibility_reasons() {
    let ruleset = ruleset();
    let candidates = vec![
        candidate("market-bad-basis", "chainlink_btcusd", 5_000.0, 900),
        CandidateMarket {
            market_id: "market-orders-closed".to_string(),
            instrument_id: "market-orders-closed-yes".to_string(),
            tag_slug: "bitcoin".to_string(),
            declared_resolution_basis: "binance_btcusdt_1m".to_string(),
            accepting_orders: false,
            liquidity_num: 5_000.0,
            seconds_to_end: 600,
        },
        candidate("market-low-liquidity", "binance_btcusdt_1m", 500.0, 600),
        candidate("market-too-soon", "binance_btcusdt_1m", 5_000.0, 60),
        candidate("market-too-late", "binance_btcusdt_1m", 5_000.0, 4_000),
        candidate("market-best", "binance_btcusdt_1m", 9_000.0, 1_200),
    ];

    let evaluation = evaluate_market_selection(&ruleset, &candidates);

    assert_eq!(
        evaluation.decision,
        SelectionDecision {
            ruleset_id: "btc-5m".to_string(),
            state: SelectionState::Active {
                market: candidate("market-best", "binance_btcusdt_1m", 9_000.0, 1_200),
            },
        }
    );
    assert_eq!(
        evaluation.rejected_candidates,
        vec![
            RejectedCandidate {
                market: candidate("market-bad-basis", "chainlink_btcusd", 5_000.0, 900),
                reason: EligibilityRejectReason::ResolutionBasisMismatch,
            },
            RejectedCandidate {
                market: CandidateMarket {
                    market_id: "market-orders-closed".to_string(),
                    instrument_id: "market-orders-closed-yes".to_string(),
                    tag_slug: "bitcoin".to_string(),
                    declared_resolution_basis: "binance_btcusdt_1m".to_string(),
                    accepting_orders: false,
                    liquidity_num: 5_000.0,
                    seconds_to_end: 600,
                },
                reason: EligibilityRejectReason::OrdersClosed,
            },
            RejectedCandidate {
                market: candidate("market-low-liquidity", "binance_btcusdt_1m", 500.0, 600),
                reason: EligibilityRejectReason::LowLiquidity,
            },
            RejectedCandidate {
                market: candidate("market-too-soon", "binance_btcusdt_1m", 5_000.0, 60),
                reason: EligibilityRejectReason::ExpiryTooSoon,
            },
            RejectedCandidate {
                market: candidate("market-too-late", "binance_btcusdt_1m", 5_000.0, 4_000),
                reason: EligibilityRejectReason::ExpiryTooLate,
            },
        ]
    );
}

#[test]
fn select_market_matches_evaluation_decision() {
    let ruleset = ruleset();
    let candidates = vec![
        candidate("market-bad-basis", "chainlink_btcusd", 5_000.0, 900),
        candidate("market-best", "binance_btcusdt_1m", 9_000.0, 1_200),
    ];

    let decision = select_market(&ruleset, &candidates);
    let evaluation = evaluate_market_selection(&ruleset, &candidates);

    assert_eq!(decision, evaluation.decision);
}

#[test]
fn eligibility_reject_reason_exposes_canonical_labels() {
    assert_eq!(
        EligibilityRejectReason::TagMismatch.as_str(),
        "tag_mismatch"
    );
    assert_eq!(
        EligibilityRejectReason::ResolutionBasisMismatch.as_str(),
        "resolution_basis_mismatch"
    );
    assert_eq!(
        EligibilityRejectReason::OrdersClosed.as_str(),
        "orders_closed"
    );
    assert_eq!(
        EligibilityRejectReason::LowLiquidity.as_str(),
        "low_liquidity"
    );
    assert_eq!(
        EligibilityRejectReason::ExpiryTooSoon.as_str(),
        "expiry_too_soon"
    );
    assert_eq!(
        EligibilityRejectReason::ExpiryTooLate.as_str(),
        "expiry_too_late"
    );
}
