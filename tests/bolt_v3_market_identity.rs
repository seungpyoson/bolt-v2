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
    bolt_v3_config::{LoadedStrategy, load_bolt_v3_config},
    bolt_v3_market_families::updown::{
        BoltV3MarketIdentityError, UpdownSlugCandidates, UpdownTargetPlan, candidates_for_target,
        plan_market_identity, updown_market_slug, updown_period_pair,
    },
};

/// Mutate a single field in the strategy's raw `[target]` TOML
/// envelope. The strategy envelope keeps `target` as a generic raw-
/// TOML container so market-family-shaped fields live in the per-
/// family binding module; tests that previously assigned to a typed
/// `TargetBlock` field reach the same effect by inserting on the
/// table.
fn set_target_field(strategy: &mut LoadedStrategy, key: &str, value: toml::Value) {
    strategy
        .config
        .target
        .as_table_mut()
        .expect("strategy [target] should be a TOML table")
        .insert(key.to_string(), value);
}

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
    set_target_field(
        &mut loaded.strategies[0],
        "cadence_seconds",
        toml::Value::Integer(120),
    );

    match plan_market_identity(&loaded) {
        Err(BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_seconds,
        }) => {
            assert_eq!(strategy_instance_id.as_deref(), Some("bitcoin_updown_main"));
            assert_eq!(configured_target_id.as_deref(), Some("btc_updown_5m"));
            assert_eq!(cadence_seconds, 120);
        }
        other => panic!("expected UnsupportedCadenceSeconds; got {other:?}"),
    }
}

#[test]
fn plan_market_identity_rejects_non_positive_cadence_seconds_after_mutation() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    set_target_field(
        &mut loaded.strategies[0],
        "cadence_seconds",
        toml::Value::Integer(0),
    );

    match plan_market_identity(&loaded) {
        Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_seconds,
        }) => {
            assert_eq!(strategy_instance_id.as_deref(), Some("bitcoin_updown_main"));
            assert_eq!(configured_target_id.as_deref(), Some("btc_updown_5m"));
            assert_eq!(cadence_seconds, 0);
        }
        other => panic!("expected NonPositiveCadenceSeconds; got {other:?}"),
    }

    let mut loaded_neg = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    set_target_field(
        &mut loaded_neg.strategies[0],
        "cadence_seconds",
        toml::Value::Integer(-300),
    );
    match plan_market_identity(&loaded_neg) {
        Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_seconds,
        }) => {
            assert_eq!(strategy_instance_id.as_deref(), Some("bitcoin_updown_main"));
            assert_eq!(configured_target_id.as_deref(), Some("btc_updown_5m"));
            assert_eq!(cadence_seconds, -300);
        }
        other => panic!("expected NonPositiveCadenceSeconds; got {other:?}"),
    }
}

#[test]
fn plan_market_identity_projects_strategies_in_declaration_order() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Construct three strategies whose declaration order is
    // deliberately NON-MONOTONIC across every likely accidental sort
    // key: strategy_instance_id, configured_target_id,
    // underlying_asset, cadence_seconds, and cadence_slug_token. Each
    // natural ordering produces a different permutation than the
    // declaration order [zeta, alpha, mike], so an accidental
    // `sort_by` on any of these keys would re-order at least one
    // index and fail the per-index assertions below.
    //
    //   declared order : [0]=zeta_strategy_main / ltc_updown_15m / LTC / 900  / 15m
    //                    [1]=alpha_strategy_main / xrp_updown_5m / XRP / 300  / 5m
    //                    [2]=mike_strategy_main / btc_updown_1h / BTC / 3600 / 1h
    //
    //   sort by strategy_instance_id ascending  -> [1, 2, 0]
    //   sort by configured_target_id ascending  -> [2, 0, 1]
    //   sort by underlying_asset ascending      -> [2, 0, 1]
    //   sort by cadence_seconds ascending       -> [1, 0, 2]
    //   sort by cadence_seconds descending      -> [2, 0, 1]
    //   sort by cadence_slug_token ascending    -> [0, 2, 1]

    let mut second = loaded.strategies[0].clone();
    let mut third = loaded.strategies[0].clone();

    {
        let first = &mut loaded.strategies[0];
        first.config.strategy_instance_id = "zeta_strategy_main".to_string();
        set_target_field(
            first,
            "configured_target_id",
            toml::Value::String("ltc_updown_15m".to_string()),
        );
        set_target_field(
            first,
            "underlying_asset",
            toml::Value::String("LTC".to_string()),
        );
        set_target_field(first, "cadence_seconds", toml::Value::Integer(900));
    }

    second.config.strategy_instance_id = "alpha_strategy_main".to_string();
    set_target_field(
        &mut second,
        "configured_target_id",
        toml::Value::String("xrp_updown_5m".to_string()),
    );
    set_target_field(
        &mut second,
        "underlying_asset",
        toml::Value::String("XRP".to_string()),
    );
    set_target_field(&mut second, "cadence_seconds", toml::Value::Integer(300));

    third.config.strategy_instance_id = "mike_strategy_main".to_string();
    set_target_field(
        &mut third,
        "configured_target_id",
        toml::Value::String("btc_updown_1h".to_string()),
    );
    set_target_field(
        &mut third,
        "underlying_asset",
        toml::Value::String("BTC".to_string()),
    );
    set_target_field(&mut third, "cadence_seconds", toml::Value::Integer(3600));

    loaded.strategies.push(second);
    loaded.strategies.push(third);

    let plan = plan_market_identity(&loaded).expect("planner should succeed for valid strategies");
    assert_eq!(plan.updown_targets.len(), 3);

    let zero = &plan.updown_targets[0];
    assert_eq!(zero.strategy_instance_id, "zeta_strategy_main");
    assert_eq!(zero.configured_target_id, "ltc_updown_15m");
    assert_eq!(zero.venue_config_key, "polymarket_main");
    assert_eq!(zero.underlying_asset, "LTC");
    assert_eq!(zero.cadence_seconds, 900);
    assert_eq!(zero.cadence_slug_token, "15m");

    let one = &plan.updown_targets[1];
    assert_eq!(one.strategy_instance_id, "alpha_strategy_main");
    assert_eq!(one.configured_target_id, "xrp_updown_5m");
    assert_eq!(one.venue_config_key, "polymarket_main");
    assert_eq!(one.underlying_asset, "XRP");
    assert_eq!(one.cadence_seconds, 300);
    assert_eq!(one.cadence_slug_token, "5m");

    let two = &plan.updown_targets[2];
    assert_eq!(two.strategy_instance_id, "mike_strategy_main");
    assert_eq!(two.configured_target_id, "btc_updown_1h");
    assert_eq!(two.venue_config_key, "polymarket_main");
    assert_eq!(two.underlying_asset, "BTC");
    assert_eq!(two.cadence_seconds, 3600);
    assert_eq!(two.cadence_slug_token, "1h");
}

#[test]
fn period_pair_overflow_display_includes_now_and_cadence_context() {
    let err = BoltV3MarketIdentityError::PeriodPairOverflow {
        now_unix_seconds: i64::MAX,
        cadence_seconds: 300,
    };
    let display = err.to_string();
    assert!(
        display.contains(&i64::MAX.to_string()),
        "Display should include now_unix_seconds value: {display}"
    );
    assert!(
        display.contains("300"),
        "Display should include cadence_seconds value: {display}"
    );
    assert!(
        display.contains("overflow"),
        "Display should describe the overflow condition: {display}"
    );
}

#[test]
fn cadence_error_display_includes_strategy_and_target_context() {
    let unsupported = BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
        strategy_instance_id: Some("bitcoin_updown_main".to_string()),
        configured_target_id: Some("btc_updown_5m".to_string()),
        cadence_seconds: 120,
    };
    let display = unsupported.to_string();
    assert!(
        display.contains("bitcoin_updown_main"),
        "Display should include strategy_instance_id: {display}"
    );
    assert!(
        display.contains("btc_updown_5m"),
        "Display should include configured_target_id: {display}"
    );
    assert!(
        display.contains("120"),
        "Display should include cadence_seconds value: {display}"
    );

    let non_positive = BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
        strategy_instance_id: Some("bitcoin_updown_main".to_string()),
        configured_target_id: Some("btc_updown_5m".to_string()),
        cadence_seconds: 0,
    };
    let np_display = non_positive.to_string();
    assert!(
        np_display.contains("bitcoin_updown_main"),
        "Display should include strategy_instance_id: {np_display}"
    );
    assert!(
        np_display.contains("btc_updown_5m"),
        "Display should include configured_target_id: {np_display}"
    );
}

#[test]
fn updown_period_pair_rejects_overflow_at_i64_max_with_supported_cadence() {
    match updown_period_pair(300, i64::MAX) {
        Err(BoltV3MarketIdentityError::PeriodPairOverflow {
            now_unix_seconds,
            cadence_seconds,
        }) => {
            assert_eq!(now_unix_seconds, i64::MAX);
            assert_eq!(cadence_seconds, 300);
        }
        other => panic!("expected PeriodPairOverflow; got {other:?}"),
    }
}

#[test]
fn candidates_for_target_propagates_period_pair_overflow() {
    let target = UpdownTargetPlan {
        strategy_instance_id: "bitcoin_updown_main".to_string(),
        configured_target_id: "btc_updown_5m".to_string(),
        venue_config_key: "polymarket_main".to_string(),
        underlying_asset: "BTC".to_string(),
        cadence_seconds: 300,
        cadence_slug_token: "5m".to_string(),
    };
    assert!(matches!(
        candidates_for_target(&target, i64::MAX),
        Err(BoltV3MarketIdentityError::PeriodPairOverflow { .. })
    ));
}

#[test]
fn core_market_identity_module_remains_provider_neutral_and_runtime_free() {
    // The core market-identity module is the product boundary that
    // future provider bindings translate into provider-shaped adapter
    // values. It must therefore stay free of every provider name and
    // every live-runtime / trading concept. Adding any of the strings
    // below to `src/bolt_v3_market_identity.rs` is a product-boundary
    // regression: provider-specific translation belongs in the adapter
    // / provider-binding layer, not in core market identity.
    let src = include_str!("../src/bolt_v3_market_identity.rs");
    let forbidden = [
        // Live-runtime / NT-runtime types
        "LiveNode",
        "Cache",
        "request_instruments",
        "connect",
        // Provider names: capitalized identifier prefix variants
        "Polymarket",
        "Binance",
        "Chainlink",
        "Gamma",
        // Provider names: lowercase identifier / docstring variants so
        // a regression like "configured polymarket venue" or a
        // `polymarket_*` snake_case identifier in core source still
        // trips the guard.
        "polymarket",
        "binance",
        "chainlink",
        "gamma",
        // Provider-specific filter type
        "MarketSlugFilter",
        // Order / execution / risk / sizing concerns: forbid both
        // snake_case (e.g. `submit_order`, `risk_engine`) and
        // CamelCase (e.g. `OrderBook`, `ExecutionEngine`) variants.
        "order",
        "Order",
        "execution",
        "Execution",
        "risk",
        "Risk",
        "sizing",
        "Sizing",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_market_identity.rs must remain provider-neutral and live-runtime-free; \
             source unexpectedly references `{symbol}`"
        );
    }
}
