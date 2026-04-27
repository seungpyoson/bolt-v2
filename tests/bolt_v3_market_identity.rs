//! Pure/control-plane market-identity tests.
//!
//! These tests guard the bolt-v3 contract that:
//!   1. Configured updown rotating-market targets project into a pure
//!      `MarketIdentityPlan` derived from validated config alone.
//!   2. The current and next updown period start values are computed
//!      from `cadence_seconds` and an injected `now_unix_seconds`, and
//!      match the runtime-contract slug-derivation rule on the
//!      boundary, one second before, and one second after.
//!   3. The updown market-slug formatter lowercases the underlying
//!      asset, uses the configured cadence slug-token, and trails the
//!      period-start unix seconds value.
//!   4. Direct struct mutation of `cadence_seconds` into an
//!      unsupported or non-positive value still fails cleanly through
//!      `plan_market_identity` rather than producing a malformed plan.
//!   5. The module source does not reference the NautilusTrader live
//!      runtime symbols this slice intentionally excludes (`LiveNode`,
//!      `connect`, `request_instruments`, `Cache`).
//!
//! Out of scope for this slice: live `LiveNode` execution, NT
//! `Cache` reads, `request_instruments`, Gamma supplement,
//! Chainlink/reference/fused price, strategy actors, or any order
//! construction. Those boundaries belong to later slices.

mod support;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_market_identity::{
        BoltV3MarketIdentityError, UpdownSlugCandidates, UpdownTargetPlan, candidates_for_target,
        plan_market_identity, updown_market_slug, updown_period_pair,
    },
};

#[test]
fn plan_market_identity_from_fixture_yields_one_updown_target_plan() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    let plan = plan_market_identity(&loaded).expect("planner should succeed for valid fixture");
    assert_eq!(plan.updown_targets.len(), 1, "one updown target plan");
    let target = &plan.updown_targets[0];
    assert_eq!(target.strategy_instance_id, "bitcoin_updown_main");
    assert_eq!(target.configured_target_id, "btc_updown_5m");
    assert_eq!(target.venue_config_key, "polymarket_main");
    assert_eq!(target.underlying_asset, "BTC");
    assert_eq!(target.cadence_seconds, 300);
    assert_eq!(target.cadence_slug_token, "5m");
}

#[test]
fn updown_period_pair_on_exact_boundary() {
    let (current, next) = updown_period_pair(300, 600).unwrap();
    assert_eq!(current, 600);
    assert_eq!(next, 900);
}

#[test]
fn updown_period_pair_one_second_before_boundary() {
    let (current, next) = updown_period_pair(300, 599).unwrap();
    assert_eq!(current, 300);
    assert_eq!(next, 600);
}

#[test]
fn updown_period_pair_one_second_after_boundary() {
    let (current, next) = updown_period_pair(300, 601).unwrap();
    assert_eq!(current, 600);
    assert_eq!(next, 900);
}

#[test]
fn updown_period_pair_at_unix_epoch_zero() {
    let (current, next) = updown_period_pair(300, 0).unwrap();
    assert_eq!(current, 0);
    assert_eq!(next, 300);
}

#[test]
fn updown_period_pair_rejects_non_positive_cadence_seconds() {
    assert!(matches!(
        updown_period_pair(0, 600),
        Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds { .. })
    ));
    assert!(matches!(
        updown_period_pair(-300, 600),
        Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds { .. })
    ));
}

#[test]
fn updown_period_pair_rejects_negative_now_unix_seconds() {
    assert!(matches!(
        updown_period_pair(300, -1),
        Err(BoltV3MarketIdentityError::NegativeNowUnixSeconds { .. })
    ));
}

#[test]
fn updown_market_slug_lowercases_asset_for_btc_5m() {
    let slug = updown_market_slug("BTC", "5m", 1_700_000_000);
    assert_eq!(slug, "btc-updown-5m-1700000000");
}

#[test]
fn updown_market_slug_lowercases_asset_for_eth_15m() {
    let slug = updown_market_slug("ETH", "15m", 1_700_000_900);
    assert_eq!(slug, "eth-updown-15m-1700000900");
}

#[test]
fn updown_market_slug_table_matches_runtime_contract() {
    let cases: &[(&str, &str, i64, &str)] = &[
        ("BTC", "1m", 1_700_000_000, "btc-updown-1m-1700000000"),
        ("BTC", "5m", 1_700_000_000, "btc-updown-5m-1700000000"),
        ("BTC", "15m", 1_700_000_000, "btc-updown-15m-1700000000"),
        ("BTC", "1h", 1_700_000_000, "btc-updown-1h-1700000000"),
        ("BTC", "4h", 1_700_000_000, "btc-updown-4h-1700000000"),
        ("ETH", "5m", 1_700_000_900, "eth-updown-5m-1700000900"),
        ("XRP", "1h", 1_700_000_000, "xrp-updown-1h-1700000000"),
        ("BTC", "5m", 0, "btc-updown-5m-0"),
    ];
    for (asset, token, period, expected) in cases {
        assert_eq!(updown_market_slug(asset, token, *period), *expected);
    }
}

#[test]
fn candidates_for_target_btc_5m_yields_current_and_next_slugs() {
    let target = UpdownTargetPlan {
        strategy_instance_id: "bitcoin_updown_main".to_string(),
        configured_target_id: "btc_updown_5m".to_string(),
        venue_config_key: "polymarket_main".to_string(),
        underlying_asset: "BTC".to_string(),
        cadence_seconds: 300,
        cadence_slug_token: "5m".to_string(),
    };
    let UpdownSlugCandidates {
        current_period_start_unix_seconds,
        next_period_start_unix_seconds,
        current_market_slug,
        next_market_slug,
    } = candidates_for_target(&target, 601).expect("candidates should succeed for valid input");
    assert_eq!(current_period_start_unix_seconds, 600);
    assert_eq!(next_period_start_unix_seconds, 900);
    assert_eq!(current_market_slug, "btc-updown-5m-600");
    assert_eq!(next_market_slug, "btc-updown-5m-900");
}

#[test]
fn candidates_for_target_propagates_negative_now_unix_seconds_error() {
    let target = UpdownTargetPlan {
        strategy_instance_id: "bitcoin_updown_main".to_string(),
        configured_target_id: "btc_updown_5m".to_string(),
        venue_config_key: "polymarket_main".to_string(),
        underlying_asset: "BTC".to_string(),
        cadence_seconds: 300,
        cadence_slug_token: "5m".to_string(),
    };
    assert!(matches!(
        candidates_for_target(&target, -1),
        Err(BoltV3MarketIdentityError::NegativeNowUnixSeconds { .. })
    ));
}

#[test]
fn plan_market_identity_rejects_unsupported_cadence_seconds_after_mutation() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Direct struct mutation: cadence=120 has no slug-token mapping in
    // the runtime-contract table.
    loaded.strategies[0].config.target.cadence_seconds = 120;

    match plan_market_identity(&loaded) {
        Err(BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
            strategy_instance_id,
            cadence_seconds,
        }) => {
            assert_eq!(strategy_instance_id.as_deref(), Some("bitcoin_updown_main"));
            assert_eq!(cadence_seconds, 120);
        }
        other => panic!("expected UnsupportedCadenceSeconds; got {other:?}"),
    }
}

#[test]
fn plan_market_identity_rejects_non_positive_cadence_seconds_after_mutation() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    loaded.strategies[0].config.target.cadence_seconds = 0;

    match plan_market_identity(&loaded) {
        Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            cadence_seconds,
        }) => {
            assert_eq!(strategy_instance_id.as_deref(), Some("bitcoin_updown_main"));
            assert_eq!(cadence_seconds, 0);
        }
        other => panic!("expected NonPositiveCadenceSeconds; got {other:?}"),
    }

    let mut loaded_neg = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded_neg.strategies[0].config.target.cadence_seconds = -300;
    match plan_market_identity(&loaded_neg) {
        Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            cadence_seconds,
        }) => {
            assert_eq!(strategy_instance_id.as_deref(), Some("bitcoin_updown_main"));
            assert_eq!(cadence_seconds, -300);
        }
        other => panic!("expected NonPositiveCadenceSeconds; got {other:?}"),
    }
}

#[test]
fn module_source_does_not_reference_forbidden_runtime_symbols() {
    let src = include_str!("../src/bolt_v3_market_identity.rs");
    let forbidden = ["LiveNode", "request_instruments", "connect", "Cache"];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "module source must not reference `{symbol}` (slice 9 is pure/control-plane only)"
        );
    }
}
