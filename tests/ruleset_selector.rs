use bolt_v2::{
    config::{RulesetConfig, RulesetVenueKind},
    platform::ruleset::{CandidateMarket, SelectionDecision, SelectionState, select_market},
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
