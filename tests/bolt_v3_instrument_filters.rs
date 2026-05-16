//! Instrument-filter tests for configured rotating-market targets.
//!
//! These tests guard the bolt-v3 contract that:
//!   1. Configured updown rotating-market targets project into a pure
//!      `InstrumentFilterConfig` derived from validated config alone.
//!   2. The current and next updown period start values are computed
//!      from `cadence_seconds` and an injected `now_unix_seconds`, and
//!      match the runtime-contract slug-derivation rule on the
//!      boundary, one second before, and one second after.
//!   3. The updown market-slug formatter lowercases the underlying
//!      asset, uses the configured cadence slug-token, and trails the
//!      period-start unix seconds value.
//!   4. Direct struct mutation of `cadence_seconds` into a non-positive
//!      value still fails cleanly through `instrument_filters_from_config` rather
//!      than producing an invalid `InstrumentFilterConfig`.
//!   5. The module source does not reference the NautilusTrader live
//!      runtime symbols this module intentionally excludes (`LiveNode`,
//!      `connect`, `request_instruments`, `Cache`).
//!
//! Out of scope: live `LiveNode` execution, NT `Cache` reads,
//! `request_instruments`, Gamma supplement, reference data, strategy
//! actors, and order construction.

mod support;

use bolt_v2::{
    bolt_v3_config::{LoadedStrategy, load_bolt_v3_config},
    bolt_v3_market_families::updown::{
        BoltV3InstrumentFilterError, UpdownInstrumentFilterTarget, UpdownSlugCandidates,
        candidates_for_target, instrument_filters_from_config, updown_market_slug,
        updown_period_pair,
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
fn instrument_filters_from_config_from_fixture_yields_one_updown_target_config() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("config should succeed for valid fixture");
    assert_eq!(
        instrument_filters.updown_targets.len(),
        1,
        "one updown target"
    );
    let target = &instrument_filters.updown_targets[0];
    assert_eq!(target.strategy_instance_id, "bitcoin_updown_main");
    assert_eq!(target.configured_target_id, "btc_updown_5m");
    assert_eq!(target.venue, "polymarket_main");
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
        Err(BoltV3InstrumentFilterError::NonPositiveCadenceSeconds { .. })
    ));
    assert!(matches!(
        updown_period_pair(-300, 600),
        Err(BoltV3InstrumentFilterError::NonPositiveCadenceSeconds { .. })
    ));
}

#[test]
fn updown_period_pair_rejects_negative_now_unix_seconds() {
    assert!(matches!(
        updown_period_pair(300, -1),
        Err(BoltV3InstrumentFilterError::NegativeNowUnixSeconds { .. })
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
    let target = UpdownInstrumentFilterTarget {
        strategy_instance_id: "bitcoin_updown_main".to_string(),
        configured_target_id: "btc_updown_5m".to_string(),
        venue: "polymarket_main".to_string(),
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
    let target = UpdownInstrumentFilterTarget {
        strategy_instance_id: "bitcoin_updown_main".to_string(),
        configured_target_id: "btc_updown_5m".to_string(),
        venue: "polymarket_main".to_string(),
        underlying_asset: "BTC".to_string(),
        cadence_seconds: 300,
        cadence_slug_token: "5m".to_string(),
    };
    assert!(matches!(
        candidates_for_target(&target, -1),
        Err(BoltV3InstrumentFilterError::NegativeNowUnixSeconds { .. })
    ));
}

#[test]
fn instrument_filters_from_config_uses_configured_cadence_slug_token() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    set_target_field(
        &mut loaded.strategies[0],
        "cadence_seconds",
        toml::Value::Integer(120),
    );
    set_target_field(
        &mut loaded.strategies[0],
        "cadence_slug_token",
        toml::Value::String("2m".to_string()),
    );

    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("configured cadence token should build");
    assert_eq!(instrument_filters.updown_targets[0].cadence_seconds, 120);
    assert_eq!(
        instrument_filters.updown_targets[0].cadence_slug_token,
        "2m"
    );
}

#[test]
fn instrument_filters_from_config_rejects_non_positive_cadence_seconds_after_mutation() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    set_target_field(
        &mut loaded.strategies[0],
        "cadence_seconds",
        toml::Value::Integer(0),
    );

    match instrument_filters_from_config(&loaded) {
        Err(BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
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
    match instrument_filters_from_config(&loaded_neg) {
        Err(BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
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
fn instrument_filters_from_config_projects_strategies_in_declaration_order() {
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
        toml::Value::String("xrp_updown_5m".to_string()),
    );
    set_target_field(
        &mut second,
        "underlying_asset",
        toml::Value::String("XRP".to_string()),
    );
    set_target_field(&mut second, "cadence_seconds", toml::Value::Integer(300));
    set_target_field(
        &mut second,
        "cadence_slug_token",
        toml::Value::String("5m".to_string()),
    );

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
    set_target_field(
        &mut third,
        "cadence_slug_token",
        toml::Value::String("1h".to_string()),
    );

    loaded.strategies.push(second);
    loaded.strategies.push(third);

    let instrument_filters = instrument_filters_from_config(&loaded)
        .expect("config should succeed for valid strategies");
    assert_eq!(instrument_filters.updown_targets.len(), 3);

    let zero = &instrument_filters.updown_targets[0];
    assert_eq!(zero.strategy_instance_id, "zeta_strategy_main");
    assert_eq!(zero.configured_target_id, "ltc_updown_15m");
    assert_eq!(zero.venue, "polymarket_main");
    assert_eq!(zero.underlying_asset, "LTC");
    assert_eq!(zero.cadence_seconds, 900);
    assert_eq!(zero.cadence_slug_token, "15m");

    let one = &instrument_filters.updown_targets[1];
    assert_eq!(one.strategy_instance_id, "alpha_strategy_main");
    assert_eq!(one.configured_target_id, "xrp_updown_5m");
    assert_eq!(one.venue, "polymarket_main");
    assert_eq!(one.underlying_asset, "XRP");
    assert_eq!(one.cadence_seconds, 300);
    assert_eq!(one.cadence_slug_token, "5m");

    let two = &instrument_filters.updown_targets[2];
    assert_eq!(two.strategy_instance_id, "mike_strategy_main");
    assert_eq!(two.configured_target_id, "btc_updown_1h");
    assert_eq!(two.venue, "polymarket_main");
    assert_eq!(two.underlying_asset, "BTC");
    assert_eq!(two.cadence_seconds, 3600);
    assert_eq!(two.cadence_slug_token, "1h");
}

#[test]
fn period_pair_overflow_display_includes_now_and_cadence_context() {
    let err = BoltV3InstrumentFilterError::PeriodPairOverflow {
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
    let non_positive = BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
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
        Err(BoltV3InstrumentFilterError::PeriodPairOverflow {
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
    let target = UpdownInstrumentFilterTarget {
        strategy_instance_id: "bitcoin_updown_main".to_string(),
        configured_target_id: "btc_updown_5m".to_string(),
        venue: "polymarket_main".to_string(),
        underlying_asset: "BTC".to_string(),
        cadence_seconds: 300,
        cadence_slug_token: "5m".to_string(),
    };
    assert!(matches!(
        candidates_for_target(&target, i64::MAX),
        Err(BoltV3InstrumentFilterError::PeriodPairOverflow { .. })
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
    // Updown cadence slug-token ownership belongs to the TOML-backed
    // updown family binding, not to core validation. Validate.rs may
    // still call into the updown family validator
    // (`bolt_v3_market_families::updown::*`) to check family-shaped
    // target fields; the substrings forbidden below pin policy
    // *ownership*, not the dispatch call itself.
    let src = include_str!("../src/bolt_v3_validate.rs");
    let forbidden = [
        // Owned table identifier and helper symbol names.
        "UPDOWN_CADENCE_SLUG_TOKEN_TABLE",
        "updown_cadence_slug_token",
        "supported_updown_cadence_seconds",
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
             Keep updown slug-token ownership out of validate.rs; have \
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
        "pub enum ArchetypeOrderType",
        "pub enum ArchetypeTimeInForce",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_config.rs must not own archetype parameter or order types; \
             source unexpectedly defines `{symbol}`. \
             Move ParametersBlock, OrderParams, ArchetypeOrderType, and \
             ArchetypeTimeInForce to \
             src/bolt_v3_archetypes/binary_oracle_edge_taker.rs; reference \
             the archetype-owned ParametersBlock from the strategy \
             envelope instead of redefining it in core config."
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
    // `[venues.<name>.{data,execution,secrets}]` block shapes belong to
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
    // dispatch identifiers still needed for family routing); the
    // concrete target-shape types — rotating-market `TargetBlock`, the
    // `RotatingMarketFamily` enum, and the `MarketSelectionRule` enum —
    // belong to a market-family binding (`crate::bolt_v3_market_families::updown`),
    // not to core config. The substrings forbidden below pin policy
    // *ownership*: a `pub struct` or `pub enum` definition for any of
    // these names in `src/bolt_v3_config.rs` is a regression. The TOML
    // field name `target` itself is lowercase and not on this list, and
    // any minimal dispatch identifier needed for family routing is
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
             it into its local TargetBlock during validation and config."
        );
    }
}

#[test]
fn validate_module_must_not_own_binary_oracle_edge_taker_policy() {
    // Bolt-v3 startup validation must stay structural and dispatch
    // strategy-archetype policy out to a dedicated archetype module.
    // The `binary_oracle_edge_taker` archetype's required reference-data
    // role and TOML parameter-shape policy belong to the archetype binding
    // (`crate::bolt_v3_archetypes::binary_oracle_edge_taker`),
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
        // Former migrated helper symbol names. The archetype no longer
        // owns a hardcoded entry/exit order-combination gate either, but
        // these names must still not return to core validation.
        "check_binary_oracle_entry_order_combination",
        "check_binary_oracle_exit_order_combination",
        "entry_order combination",
        "exit_order combination",
        // Concrete archetype-required reference-data error-message phrase.
        "[reference_data.<role>]",
    ];
    for symbol in forbidden {
        assert!(
            !src.contains(symbol),
            "src/bolt_v3_validate.rs must not own binary_oracle_edge_taker policy; \
             source unexpectedly references `{symbol}`. \
             Move the archetype's required reference-data role, its \
             entry/exit order-combination rules, and the matching error \
             messages to src/bolt_v3_archetypes/binary_oracle_edge_taker.rs; \
             have validate.rs dispatch into the archetype validator via \
             the `bolt_v3_archetypes` namespace instead."
        );
    }
}

#[test]
fn validate_module_must_not_own_provider_venue_validation() {
    // Bolt-v3 startup validation must stay provider-neutral and
    // dispatch provider-specific venue-block validation out to the
    // per-provider binding modules. The validation policy for
    // Polymarket and Binance venue blocks (data/execution/secrets
    // shape rules, EVM funder-address syntax, retry-bounds ordering,
    // controlled-connect invariant for `subscribe_new_markets`,
    // per-provider secret-path ownership, base-URL emptiness,
    // instrument-status-poll positivity) belongs to the per-provider
    // binding modules under `crate::bolt_v3_providers`, not to core
    // validation. Validate.rs may still hand the venue block to a
    // family-agnostic provider dispatcher
    // (`bolt_v3_providers::validate_venue_block`) for routing; the
    // substrings forbidden below pin policy *ownership* (function
    // definitions and provider-shaped block types referenced by
    // those validators), not the dispatch call itself.
    let src = include_str!("../src/bolt_v3_validate.rs");
    let forbidden = [
        // Per-provider venue-block validators that owned the policy
        // before provider bindings.
        "validate_polymarket_venue",
        "validate_binance_venue",
        // Polymarket execution-shape policy.
        "validate_polymarket_funder_address",
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
            "src/bolt_v3_validate.rs must not own provider-specific venue validation; \
             source unexpectedly references `{symbol}`. \
             Move Polymarket / Binance venue, data, execution, funder-address, \
             retry-bounds, secret-path, and EVM-syntax validators (and the \
             provider-shaped block types they consume) into \
             src/bolt_v3_providers/polymarket.rs and src/bolt_v3_providers/binance.rs; \
             have validate.rs dispatch into the provider validator via \
             `bolt_v3_providers::validate_venue_block` instead."
        );
    }
}
