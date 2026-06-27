//! Pure/control-plane market-identity tests.
//!
//! These tests guard the bolt-v3 contract that:
//!   1. Configured updown rotating-market targets project into a pure
//!      `MarketIdentityPlan` derived from validated config alone.
//!   2. The current and next updown period start values are computed
//!      from `cadence_secs` and an injected `now_unix_secs`, and
//!      match the configured slug-token on the boundary, one second
//!      before, and one second after.
//!   3. The updown market-slug formatter lowercases the underlying
//!      asset, uses the configured cadence slug-token, and trails the
//!      period-start unix seconds value.
//!   4. Direct struct mutation of cadence fields into invalid values
//!      still fails cleanly through `plan_market_identity` rather than
//!      producing a malformed plan.
//!   5. The module source does not reference the NautilusTrader live
//!      runtime symbols this slice intentionally excludes (`LiveNode`,
//!      `connect`, `request_instruments`, `Cache`).
//!
//! Out of scope for this slice: live `LiveNode` execution, NT
//! `Cache` reads, `request_instruments`, Gamma supplement,
//! Chainlink/reference/fused price, strategy actors, or any order
//! construction. Those boundaries belong to later slices.

use crate::support;

use bolt_v2::{
    bolt_v3_config::{LoadedStrategy, load_bolt_v3_config},
    bolt_v3_market_families::{
        market_identity_plan_from_config,
        updown::{
            BoltV3MarketIdentityError, UpdownSlugCandidates, UpdownTargetPlan,
            candidates_for_target, plan_market_identity, target_plans, updown_market_slug,
            updown_period_pair,
        },
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
fn market_identity_plan_accepts_hyperliquid_static_instrument_target() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy = loaded
        .strategies
        .get_mut(0)
        .expect("fixture should contain one strategy");
    strategy.config.execution_client_id = "hyperliquid_perps".into();
    {
        let target = strategy
            .config
            .target
            .as_table_mut()
            .expect("strategy [target] should be a TOML table");
        for key in [
            "underlying_asset",
            "cadence_secs",
            "cadence_slug_token",
            "market_selection_rule",
            "retry_interval_secs",
            "blocked_after_secs",
            "gate_subscriptions",
        ] {
            target.remove(key);
        }
    }
    set_target_field(
        strategy,
        "configured_target_id",
        toml::Value::String("configured_hyperliquid_btc_perp".to_string()),
    );
    set_target_field(
        strategy,
        "kind",
        toml::Value::String("static_instrument".to_string()),
    );
    set_target_field(
        strategy,
        "rotating_market_family",
        toml::Value::String("hyperliquid_instrument".to_string()),
    );
    set_target_field(
        strategy,
        "product_surface",
        toml::Value::String("standard_perps".to_string()),
    );
    set_target_field(
        strategy,
        "instrument_id",
        toml::Value::String("BTC-PERP.HYPERLIQUID".to_string()),
    );
    set_target_field(
        strategy,
        "quantity_step",
        toml::Value::String("0.001".to_string()),
    );

    let plan = market_identity_plan_from_config(&loaded)
        .expect("hyperliquid static-instrument target should be a supported family");
    let refs = plan.execution_client_target_refs().collect::<Vec<_>>();
    assert_eq!(refs.len(), 1, "one direct Hyperliquid target ref");
    assert_eq!(refs[0].family_key, "hyperliquid_instrument");
    assert_eq!(
        refs[0].configured_target_id,
        "configured_hyperliquid_btc_perp"
    );
    assert_eq!(refs[0].execution_client_id, "hyperliquid_perps");
}

#[test]
fn plan_market_identity_from_fixture_yields_one_updown_target_plan() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    let plan = plan_market_identity(&loaded).expect("planner should succeed for valid fixture");
    let targets = target_plans(&plan).collect::<Vec<_>>();
    assert_eq!(targets.len(), 1, "one updown target plan");
    let target = targets[0];
    assert_eq!(target.strategy_instance_id, "configured_updown_main");
    assert_eq!(target.configured_target_id, "configured_updown_target");
    assert_eq!(target.execution_client_id, "polymarket_main");
    assert_eq!(target.underlying_asset, "CONFIGURED_ASSET");
    assert_eq!(target.cadence_secs, 300);
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
fn updown_period_pair_rejects_non_positive_cadence_secs() {
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
fn updown_period_pair_rejects_negative_now_unix_secs() {
    assert!(matches!(
        updown_period_pair(300, -1),
        Err(BoltV3MarketIdentityError::NegativeNowUnixSeconds { .. })
    ));
}

#[test]
fn updown_market_slug_lowercases_configured_asset() {
    let slug = updown_market_slug("ASSET", "window", 1_700_000_000);
    assert_eq!(slug, "asset-updown-window-1700000000");
}

#[test]
fn updown_market_slug_accepts_distinct_configured_tokens() {
    let slug = updown_market_slug("ALT", "longwindow", 1_700_000_900);
    assert_eq!(slug, "alt-updown-longwindow-1700000900");
}

#[test]
fn updown_market_slug_uses_configured_token_without_asset_assumptions() {
    let cases: &[(&str, &str, i64, &str)] = &[
        (
            "ALPHA",
            "shortwindow",
            1_700_000_000,
            "alpha-updown-shortwindow-1700000000",
        ),
        (
            "BETA",
            "mediumwindow",
            1_700_000_900,
            "beta-updown-mediumwindow-1700000900",
        ),
        ("GAMMA", "longwindow", 0, "gamma-updown-longwindow-0"),
    ];
    for (asset, token, period, expected) in cases {
        assert_eq!(updown_market_slug(asset, token, *period), *expected);
    }
}

#[test]
fn candidates_for_target_yields_current_and_next_configured_slugs() {
    let target = UpdownTargetPlan {
        strategy_instance_id: "configured_updown_main".to_string(),
        configured_target_id: "configured_updown_target".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        underlying_asset: "ASSET".to_string(),
        cadence_secs: 300,
        cadence_slug_token: "5m".to_string(),
    };
    let UpdownSlugCandidates {
        current_period_start_unix_secs,
        next_period_start_unix_secs,
        current_market_slug,
        next_market_slug,
    } = candidates_for_target(&target, 601).expect("candidates should succeed for valid input");
    assert_eq!(current_period_start_unix_secs, 600);
    assert_eq!(next_period_start_unix_secs, 900);
    assert_eq!(current_market_slug, "asset-updown-5m-600");
    assert_eq!(next_market_slug, "asset-updown-5m-900");
}

#[test]
fn candidates_for_target_propagates_negative_now_unix_seconds_error() {
    let target = UpdownTargetPlan {
        strategy_instance_id: "configured_updown_main".to_string(),
        configured_target_id: "configured_updown_target".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        underlying_asset: "ASSET".to_string(),
        cadence_secs: 300,
        cadence_slug_token: "5m".to_string(),
    };
    assert!(matches!(
        candidates_for_target(&target, -1),
        Err(BoltV3MarketIdentityError::NegativeNowUnixSeconds { .. })
    ));
}

#[test]
fn plan_market_identity_rejects_invalid_cadence_slug_token_after_mutation() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    set_target_field(
        &mut loaded.strategies[0],
        "cadence_slug_token",
        toml::Value::String("Bad-Token".to_string()),
    );

    match plan_market_identity(&loaded) {
        Err(BoltV3MarketIdentityError::InvalidCadenceSlugToken {
            strategy_instance_id,
            configured_target_id,
            cadence_slug_token,
        }) => {
            assert_eq!(
                strategy_instance_id.as_deref(),
                Some("configured_updown_main")
            );
            assert_eq!(
                configured_target_id.as_deref(),
                Some("configured_updown_target")
            );
            assert_eq!(cadence_slug_token, "Bad-Token");
        }
        other => panic!("expected InvalidCadenceSlugToken; got {other:?}"),
    }
}

#[test]
fn plan_market_identity_rejects_cadence_slug_token_contract_mismatch_after_mutation() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    set_target_field(
        &mut loaded.strategies[0],
        "cadence_slug_token",
        toml::Value::String("configuredwindow".to_string()),
    );

    match plan_market_identity(&loaded) {
        Err(BoltV3MarketIdentityError::CadenceSlugTokenMismatch {
            strategy_instance_id,
            configured_target_id,
            cadence_secs,
            cadence_slug_token,
            expected_cadence_slug_token,
        }) => {
            assert_eq!(
                strategy_instance_id.as_deref(),
                Some("configured_updown_main")
            );
            assert_eq!(
                configured_target_id.as_deref(),
                Some("configured_updown_target")
            );
            assert_eq!(cadence_secs, 300);
            assert_eq!(cadence_slug_token, "configuredwindow");
            assert_eq!(expected_cadence_slug_token, "5m");
        }
        other => panic!("expected CadenceSlugTokenMismatch; got {other:?}"),
    }
}

#[test]
fn plan_market_identity_rejects_non_positive_cadence_seconds_after_mutation() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    set_target_field(
        &mut loaded.strategies[0],
        "cadence_secs",
        toml::Value::Integer(0),
    );

    match plan_market_identity(&loaded) {
        Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_secs,
        }) => {
            assert_eq!(
                strategy_instance_id.as_deref(),
                Some("configured_updown_main")
            );
            assert_eq!(
                configured_target_id.as_deref(),
                Some("configured_updown_target")
            );
            assert_eq!(cadence_secs, 0);
        }
        other => panic!("expected NonPositiveCadenceSeconds; got {other:?}"),
    }

    let mut loaded_neg = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    set_target_field(
        &mut loaded_neg.strategies[0],
        "cadence_secs",
        toml::Value::Integer(-300),
    );
    match plan_market_identity(&loaded_neg) {
        Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_secs,
        }) => {
            assert_eq!(
                strategy_instance_id.as_deref(),
                Some("configured_updown_main")
            );
            assert_eq!(
                configured_target_id.as_deref(),
                Some("configured_updown_target")
            );
            assert_eq!(cadence_secs, -300);
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
    // underlying_asset, cadence_secs, and cadence_slug_token. Each
    // natural ordering produces a different permutation than the
    // declaration order [zeta, alpha, mike], so an accidental
    // `sort_by` on any of these keys would re-order at least one
    // index and fail the per-index assertions below.
    //
    //   declared order : [0]=zeta_strategy_main / zeta_target / ZETA / 900  / 15m
    //                    [1]=alpha_strategy_main / alpha_target / ALPHA / 300 / 5m
    //                    [2]=mike_strategy_main / mike_target / MIKE / 3600 / 1h
    //
    //   sort by strategy_instance_id ascending  -> [1, 2, 0]
    //   sort by configured_target_id ascending  -> [1, 2, 0]
    //   sort by underlying_asset ascending      -> [1, 2, 0]
    //   sort by cadence_secs ascending       -> [1, 0, 2]
    //   sort by cadence_secs descending      -> [2, 0, 1]
    //   sort by cadence_slug_token ascending    -> [0, 2, 1]

    let mut second = loaded.strategies[0].clone();
    let mut third = loaded.strategies[0].clone();

    {
        let first = &mut loaded.strategies[0];
        first.config.strategy_instance_id = "zeta_strategy_main".to_string();
        set_target_field(
            first,
            "configured_target_id",
            toml::Value::String("zeta_target".to_string()),
        );
        set_target_field(
            first,
            "underlying_asset",
            toml::Value::String("ZETA".to_string()),
        );
        set_target_field(first, "cadence_secs", toml::Value::Integer(900));
        set_target_field(
            first,
            "cadence_slug_token",
            toml::Value::String("15m".to_string()),
        );
    }

    second.config.strategy_instance_id = "alpha_strategy_main".to_string();
    set_target_field(
        &mut second,
        "configured_target_id",
        toml::Value::String("alpha_target".to_string()),
    );
    set_target_field(
        &mut second,
        "underlying_asset",
        toml::Value::String("ALPHA".to_string()),
    );
    set_target_field(&mut second, "cadence_secs", toml::Value::Integer(300));
    set_target_field(
        &mut second,
        "cadence_slug_token",
        toml::Value::String("5m".to_string()),
    );

    third.config.strategy_instance_id = "mike_strategy_main".to_string();
    set_target_field(
        &mut third,
        "configured_target_id",
        toml::Value::String("mike_target".to_string()),
    );
    set_target_field(
        &mut third,
        "underlying_asset",
        toml::Value::String("MIKE".to_string()),
    );
    set_target_field(&mut third, "cadence_secs", toml::Value::Integer(3600));
    set_target_field(
        &mut third,
        "cadence_slug_token",
        toml::Value::String("1h".to_string()),
    );

    loaded.strategies.push(second);
    loaded.strategies.push(third);

    let plan = plan_market_identity(&loaded).expect("planner should succeed for valid strategies");
    let targets = target_plans(&plan).collect::<Vec<_>>();
    assert_eq!(targets.len(), 3);

    let zero = targets[0];
    assert_eq!(zero.strategy_instance_id, "zeta_strategy_main");
    assert_eq!(zero.configured_target_id, "zeta_target");
    assert_eq!(zero.execution_client_id, "polymarket_main");
    assert_eq!(zero.underlying_asset, "ZETA");
    assert_eq!(zero.cadence_secs, 900);
    assert_eq!(zero.cadence_slug_token, "15m");

    let one = targets[1];
    assert_eq!(one.strategy_instance_id, "alpha_strategy_main");
    assert_eq!(one.configured_target_id, "alpha_target");
    assert_eq!(one.execution_client_id, "polymarket_main");
    assert_eq!(one.underlying_asset, "ALPHA");
    assert_eq!(one.cadence_secs, 300);
    assert_eq!(one.cadence_slug_token, "5m");

    let two = targets[2];
    assert_eq!(two.strategy_instance_id, "mike_strategy_main");
    assert_eq!(two.configured_target_id, "mike_target");
    assert_eq!(two.execution_client_id, "polymarket_main");
    assert_eq!(two.underlying_asset, "MIKE");
    assert_eq!(two.cadence_secs, 3600);
    assert_eq!(two.cadence_slug_token, "1h");
}

#[test]
fn period_pair_overflow_display_includes_now_and_cadence_context() {
    let err = BoltV3MarketIdentityError::PeriodPairOverflow {
        now_unix_secs: i64::MAX,
        cadence_secs: 300,
    };
    let display = err.to_string();
    assert!(
        display.contains(&i64::MAX.to_string()),
        "Display should include now_unix_secs value: {display}"
    );
    assert!(
        display.contains("300"),
        "Display should include cadence_secs value: {display}"
    );
    assert!(
        display.contains("overflow"),
        "Display should describe the overflow condition: {display}"
    );
}

#[test]
fn cadence_error_display_includes_strategy_and_target_context() {
    let invalid_slug = BoltV3MarketIdentityError::InvalidCadenceSlugToken {
        strategy_instance_id: Some("configured_updown_main".to_string()),
        configured_target_id: Some("configured_updown_target".to_string()),
        cadence_slug_token: "Bad-Token".to_string(),
    };
    let display = invalid_slug.to_string();
    assert!(
        display.contains("configured_updown_main"),
        "Display should include strategy_instance_id: {display}"
    );
    assert!(
        display.contains("configured_updown_target"),
        "Display should include configured_target_id: {display}"
    );
    assert!(
        display.contains("Bad-Token"),
        "Display should include cadence_slug_token value: {display}"
    );

    let non_positive = BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
        strategy_instance_id: Some("configured_updown_main".to_string()),
        configured_target_id: Some("configured_updown_target".to_string()),
        cadence_secs: 0,
    };
    let np_display = non_positive.to_string();
    assert!(
        np_display.contains("configured_updown_main"),
        "Display should include strategy_instance_id: {np_display}"
    );
    assert!(
        np_display.contains("configured_updown_target"),
        "Display should include configured_target_id: {np_display}"
    );
}

#[test]
fn updown_period_pair_rejects_overflow_at_i64_max_with_supported_cadence() {
    match updown_period_pair(300, i64::MAX) {
        Err(BoltV3MarketIdentityError::PeriodPairOverflow {
            now_unix_secs,
            cadence_secs,
        }) => {
            assert_eq!(now_unix_secs, i64::MAX);
            assert_eq!(cadence_secs, 300);
        }
        other => panic!("expected PeriodPairOverflow; got {other:?}"),
    }
}

#[test]
fn candidates_for_target_propagates_period_pair_overflow() {
    let target = UpdownTargetPlan {
        strategy_instance_id: "configured_updown_main".to_string(),
        configured_target_id: "configured_updown_target".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        underlying_asset: "ASSET".to_string(),
        cadence_secs: 300,
        cadence_slug_token: "5m".to_string(),
    };
    assert!(matches!(
        candidates_for_target(&target, i64::MAX),
        Err(BoltV3MarketIdentityError::PeriodPairOverflow { .. })
    ));
}

#[test]
fn core_instrument_filters_module_does_not_import_provider_or_runtime() {
    // `src/bolt_v3_instrument_filters.rs` stores only configured
    // target fields. Provider names, NT live runtime types, and trading
    // terms belong in provider, runtime, or strategy modules.
    let src = include_str!("../src/bolt_v3_instrument_filters.rs");
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
        // Order / risk / sizing concerns: forbid both
        // snake_case (e.g. `submit_order`, `risk_engine`) and
        // CamelCase (e.g. `OrderBook`, `RiskEngine`) variants.
        "order",
        "Order",
        "risk",
        "Risk",
        "sizing",
        "Sizing",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_instrument_filters.rs must not import provider or runtime terms; \
             source unexpectedly references `{symbol}`"
        );
    }
}

#[test]
fn core_instrument_filters_module_does_not_import_provider_code() {
    // No specific data or venue provider name may appear in this
    // module. Provider-specific translation belongs in provider
    // bindings.
    let src = include_str!("../src/bolt_v3_instrument_filters.rs");
    let forbidden = [
        "Polymarket",
        "polymarket",
        "Binance",
        "binance",
        "Gamma",
        "gamma",
        "Chainlink",
        "chainlink",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_instrument_filters.rs must not import provider code; \
             source unexpectedly references `{symbol}`. \
             Provider-specific translation belongs in provider bindings."
        );
    }
}

#[test]
fn core_instrument_filters_module_does_not_import_family_construction_code() {
    // `InstrumentFilterConfig` may carry configured target fields that
    // family bindings derived from TOML, but family modules own
    // parsing, validation, and filter construction.
    let src = include_str!("../src/bolt_v3_instrument_filters.rs");
    let forbidden = [
        "bolt_v3_market_families",
        "crate::bolt_v3_market_families",
        "updown::",
        "UpdownInstrumentFilterConfig",
        "UpdownInstrumentFilterTarget",
        "UpdownSlugCandidates",
        "updown_market_slug",
        "updown_period_pair",
        "MarketSlugFilter",
        "RotatingMarket",
        "RotatingMarketFamily",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_instrument_filters.rs must not import family construction code; \
             source unexpectedly references `{symbol}`. \
             Family-specific parsing, validation, and NT filter construction \
             belong in bolt_v3_market_families and provider bindings."
        );
    }
}

#[test]
fn core_instrument_filters_module_does_not_import_strategy_policy_code() {
    // Current/next selection and strategy-specific names belong in
    // strategy modules, not in configured target fields.
    let src = include_str!("../src/bolt_v3_instrument_filters.rs");
    let forbidden = [
        // Current-or-next candidate selection: identifier forms used
        // by `UpdownSlugCandidates`, plus strategy-policy names owned
        // outside the instrument-filter config module.
        "current_market_slug",
        "next_market_slug",
        "current_period_start_unix_seconds",
        "next_period_start_unix_seconds",
        "active_or_next",
        "ActiveOrNext",
        // Strategy archetypes (binary oracle edge-taker and similar).
        "binary_oracle_edge_taker",
        "BinaryOracleEdgeTaker",
        "edge_taker",
        "EdgeTaker",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_instrument_filters.rs must not import strategy policy code; \
             source unexpectedly references `{symbol}`. \
             Current/next candidate selection and strategy archetypes \
             belong in strategy modules."
        );
    }
}

#[test]
fn validate_module_must_not_own_updown_slug_token_policy() {
    // Bolt-v3 startup validation must stay structural and dispatch
    // family-specific policy out to the per-family binding module.
    // Updown cadence slug-token validation belongs to the updown
    // family binding, not to core validation. Validate.rs may still
    // call into the
    // updown family validator (`bolt_v3_market_families::updown::*`)
    // to check family-shaped target fields; the substrings forbidden
    // below pin policy *ownership*, not the dispatch call itself.
    let src = include_str!("../src/bolt_v3_validate.rs");
    let forbidden = [
        // Deprecated code-owned table identifier and helper symbol names.
        "UPDOWN_CADENCE_SLUG_TOKEN_TABLE",
        "updown_cadence_slug_token",
        "supported_updown_cadence_secs",
        // Updown slug-token error/message policy phrase, in both the
        // hyphenated prose form used in error messages and the
        // snake_case identifier form.
        "slug-token",
        "slug_token",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_validate.rs must not own updown slug-token policy; \
             source unexpectedly references `{symbol}`. \
             Keep updown slug-token validation and error messaging in \
             src/bolt_v3_market_families/updown.rs; have \
             validate.rs dispatch into the updown family validator instead."
        );
    }
}

#[test]
fn config_module_must_not_hard_type_parameters_field_to_one_archetype() {
    // Bolt-v3 root/strategy config envelope must stay archetype-neutral
    // even at the field-type level. The strategy envelope keeps the
    // TOML field name `parameters` (lowercase, allowed below) but its
    // Rust type must be a generic raw-TOML container — the concrete
    // archetype-shaped `ParametersBlock` must not appear in
    // `src/bolt_v3_config.rs`, and the envelope must not import or
    // path-reference the per-archetype binding module
    // (`binary_oracle_edge_taker`). The substrings forbidden below pin
    // that neutrality: a `pub parameters: ParametersBlock`-style
    // declaration or a `crate::bolt_v3_archetypes::binary_oracle_edge_taker::*`
    // path in core config is a regression. Note: the field name
    // `parameters` itself is lowercase and not on this list, and the
    // archetype dispatch identifier `StrategyArchetype::BinaryOracleEdgeTaker`
    // (PascalCase, not snake_case) is also intentionally not listed.
    let src = include_str!("../src/bolt_v3_config.rs");
    let forbidden = ["ParametersBlock", "binary_oracle_edge_taker"];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_config.rs must not hard-type `parameters` to one archetype's parameter row \
             or path-reference the per-archetype binding module; \
             source unexpectedly references `{symbol}`. \
             Type the strategy envelope's `parameters` field as a generic raw-TOML \
             container (`toml::Value`) and have \
             `crate::bolt_v3_archetypes::binary_oracle_edge_taker` deserialize it \
             into its local ParametersBlock during validation, dispatched via \
             `StrategyArchetype`."
        );
    }
}

#[test]
fn config_module_must_not_own_archetype_parameter_or_order_types() {
    // Bolt-v3 root/strategy config envelope must stay archetype-neutral
    // and dispatch archetype-shaped `[parameters]` / `[parameters.*]`
    // block types out to the per-archetype binding module. The config
    // module owns the strategy envelope (including the field name
    // `parameters` and the dispatch identifier
    // `StrategyArchetype::BinaryOracleEdgeTaker`); the concrete shape
    // of the `[parameters]` block, the `[parameters.entry_order]` /
    // `[parameters.exit_order]` rows, and the order-type / time-in-
    // force enums all belong to the archetype binding
    // (`crate::bolt_v3_archetypes::binary_oracle_edge_taker`). The
    // forbidden substrings below pin policy *ownership*: a `pub struct`
    // or `pub enum` definition for any of these names in
    // `src/bolt_v3_config.rs` is a regression. The strategy envelope
    // may still *reference* the archetype-owned `ParametersBlock` by
    // path or via a `use` statement so the existing TOML schema keeps
    // working — only the local definition is forbidden.
    let src = include_str!("../src/bolt_v3_config.rs");
    let forbidden = [
        "pub struct ParametersBlock",
        "pub struct OrderParams",
        // Shadow order-type / time-in-force enum definitions are forbidden
        // entirely: archetype-shaped rows now use NT's canonical
        // `nautilus_model::enums::{OrderType, TimeInForce}` directly.
        "pub enum ArchetypeOrderType",
        "pub enum ArchetypeTimeInForce",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_config.rs must not own archetype parameter or order types; \
             source unexpectedly defines `{symbol}`. \
             ParametersBlock and OrderParams belong in \
             src/bolt_v3_archetypes/binary_oracle_edge_taker.rs and reference \
             NT's canonical OrderType / TimeInForce enums; reference the \
             archetype-owned ParametersBlock from the strategy envelope instead \
             of redefining it in core config."
        );
    }
}

#[test]
fn config_module_must_not_own_provider_specific_config_block_types() {
    // Bolt-v3 root/strategy config envelope must stay provider-neutral
    // and dispatch provider-specific block shapes out to the per-
    // provider binding modules. The config module owns the root and
    // strategy envelope plus minimal dispatch identifiers like
    // `VenueKind::Polymarket` / `VenueKind::Binance`; concrete
    // `[clients.<name>.{data,execution,secrets}]` block shapes belong to
    // a per-provider binding (`crate::bolt_v3_providers::polymarket` or
    // `crate::bolt_v3_providers::binance`), not to core config. The
    // type names forbidden below pin policy *ownership* — none of these
    // provider config block types may be defined or otherwise named in
    // `src/bolt_v3_config.rs`. Note: this guard is deliberately scoped
    // to provider config block *types* only; minimal dispatch
    // identifiers like `VenueKind::Polymarket` / `VenueKind::Binance`
    // remain in core config and are not forbidden here.
    let src = include_str!("../src/bolt_v3_config.rs");
    let forbidden = [
        // Polymarket per-block config types.
        "PolymarketDataConfig",
        "PolymarketExecutionConfig",
        "PolymarketSignatureType",
        "PolymarketSecretsConfig",
        // Binance per-block config types.
        "BinanceDataConfig",
        "BinanceProductType",
        "BinanceEnvironment",
        "BinanceSecretsConfig",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_config.rs must not own provider-specific config block types; \
             source unexpectedly references `{symbol}`. \
             Move Polymarket data/execution/secrets/signature types to \
             src/bolt_v3_providers/polymarket.rs and Binance \
             data/secrets/product/environment types to \
             src/bolt_v3_providers/binance.rs; have validate, secrets, \
             and adapters import the moved types from the \
             `bolt_v3_providers` namespace instead."
        );
    }
}

#[test]
fn config_module_must_not_own_market_family_target_types() {
    // Bolt-v3 root/strategy config envelope must stay market-family-
    // neutral and dispatch market-family-shaped target block types out
    // to the per-family binding modules. The config module owns the
    // strategy envelope (including the field name `target` and minimal
    // dispatch identifiers if still needed during this slice); the
    // concrete target-shape types — rotating-market `TargetBlock`, the
    // `RotatingMarketFamily` enum, and the `MarketSelectionRule` enum —
    // belong to a market-family binding (`crate::bolt_v3_market_families::updown`),
    // not to core config. The substrings forbidden below pin policy
    // *ownership*: a `pub struct` or `pub enum` definition for any of
    // these names in `src/bolt_v3_config.rs` is a regression. The TOML
    // field name `target` itself is lowercase and not on this list, and
    // any minimal dispatch identifier needed during this slice is
    // intentionally not forbidden either.
    let src = include_str!("../src/bolt_v3_config.rs");
    let forbidden = [
        "pub struct TargetBlock",
        "pub enum RotatingMarketFamily",
        "pub enum MarketSelectionRule",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_config.rs must not own market-family target-shape types; \
             source unexpectedly defines `{symbol}`. \
             Move TargetBlock, RotatingMarketFamily, and MarketSelectionRule \
             to src/bolt_v3_market_families/updown.rs; type the strategy \
             envelope's `target` field as a generic raw-TOML container \
             (`toml::Value`) and have the updown family binding deserialize \
             it into its local TargetBlock during validation and planning."
        );
    }
}

#[test]
fn validate_module_must_not_own_binary_oracle_edge_taker_policy() {
    // Bolt-v3 startup validation must stay structural and dispatch
    // strategy-archetype policy out to a dedicated archetype module.
    // The `binary_oracle_edge_taker` archetype's required reference-data
    // role, its allowed entry/exit order-combination rules, and the
    // error-message policy that names those rules belong to the
    // archetype binding (`crate::bolt_v3_archetypes::binary_oracle_edge_taker`),
    // not to core validation. Validate.rs may still dispatch into the
    // archetype validator through the `bolt_v3_archetypes` namespace;
    // the substrings forbidden below pin policy *ownership*, not the
    // dispatch call itself.
    let src = include_str!("../src/bolt_v3_validate.rs");
    let forbidden = [
        // Archetype identifier in snake_case (error messages, helper
        // names, module-leaf paths) and PascalCase (enum variant). The
        // dispatcher in `bolt_v3_archetypes::mod` owns the match on
        // `StrategyArchetype::BinaryOracleEdgeTaker`, so neither casing
        // needs to appear in core validation.
        "binary_oracle_edge_taker",
        "BinaryOracleEdgeTaker",
        // Migrated helper symbol names.
        "check_binary_oracle_entry_order_combination",
        "check_binary_oracle_exit_order_combination",
        // Concrete entry/exit order-combination error-message phrases
        // (both the "is not allowed" headline and the per-field rule
        // listings that name `order_type=limit/market` and
        // `time_in_force=fok/ioc`).
        "entry_order combination",
        "exit_order combination",
        "order_type=limit",
        "order_type=market",
        "time_in_force=fok",
        "time_in_force=ioc",
        // Concrete archetype-required reference-current-price error-message phrase.
        "[reference_current_price]",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_validate.rs must not own binary_oracle_edge_taker policy; \
             source unexpectedly references `{symbol}`. \
             Move the archetype's required reference-current-price role, its \
             entry/exit order-combination rules, and the matching error \
             messages to src/bolt_v3_archetypes/binary_oracle_edge_taker.rs; \
             have validate.rs dispatch into the archetype validator via \
             the `bolt_v3_archetypes` namespace instead."
        );
    }
}

#[test]
fn validate_module_must_not_own_provider_client_validation() {
    // Bolt-v3 startup validation must stay provider-neutral and
    // dispatch provider-specific client-block validation out to the
    // per-provider binding modules. The validation policy for
    // Polymarket and Binance client blocks (data/execution/secrets
    // shape rules, EVM funder-address syntax, retry-bounds ordering,
    // controlled-connect invariant for `subscribe_new_markets`,
    // per-provider secret-path ownership, base-URL emptiness,
    // instrument-status-poll positivity) belongs to the per-provider
    // binding modules under `crate::bolt_v3_providers`, not to core
    // validation. Validate.rs may still hand the client block to a
    // family-agnostic provider dispatcher
    // (`bolt_v3_providers::validate_client_block`) for routing; the
    // substrings forbidden below pin policy *ownership* (function
    // definitions and provider-shaped block types referenced by
    // those validators), not the dispatch call itself.
    let src = include_str!("../src/bolt_v3_validate.rs");
    let forbidden = [
        // Per-provider client-block validators that owned the policy
        // before this slice.
        "validate_polymarket_venue",
        "validate_binance_venue",
        // Polymarket execution-shape policy.
        "validate_polymarket_funder",
        "check_evm_address_syntax",
        // Provider data/execution bounds policy.
        "validate_polymarket_data_bounds",
        "validate_polymarket_execution_bounds",
        "validate_binance_data_bounds",
        // Provider secret-path policy.
        "validate_polymarket_secret_paths",
        "validate_binance_secret_paths",
        // Provider-shaped config block types consumed only by the
        // per-provider validators. After the move core validation
        // does not need these in scope.
        "PolymarketDataConfig",
        "PolymarketExecutionConfig",
        "PolymarketSecretsConfig",
        "PolymarketSignatureType",
        "BinanceDataConfig",
        "BinanceSecretsConfig",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_validate.rs must not own provider-specific client validation; \
             source unexpectedly references `{symbol}`. \
             Move Polymarket / Binance client, data, execution, funder-address, \
             retry-bounds, secret-path, and EVM-syntax validators (and the \
             provider-shaped block types they consume) into \
             src/bolt_v3_providers/polymarket.rs and src/bolt_v3_providers/binance.rs; \
             have validate.rs dispatch into the provider validator via \
             `bolt_v3_providers::validate_client_block` instead."
        );
    }
}
