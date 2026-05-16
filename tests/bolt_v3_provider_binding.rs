//! Provider-binding tests for bolt-v3.
//!
//! These tests guard the NT adapter mapping from configured strategy
//! targets into provider-owned instrument filters. For Polymarket, the
//! mapped value is a `MarketSlugFilter` installed on
//! `PolymarketDataClientConfig.filters`.
//!
//! What these tests prove:
//!   1. The new instrument-filter-aware mapper installs exactly one
//!      provider filter per configured updown target on the matching
//!      venue, and the filter yields `[current_slug, next_slug]` for
//!      the injected fixed clock.
//!   2. Multi-target filter ordering follows declared strategy
//!      sequence and never reorders by an accidental sort key.
//!   3. `subscribe_new_markets` remains a configured NT data-client
//!      value through the instrument-filter entry point.
//!   4. An empty `InstrumentFilterConfig` installs no provider filter,
//!      preserving the previous default behaviour for non-rotating
//!      configurations.
//!
//! Out of scope: live `LiveNode` runtime, NT cache reads,
//! `request_instruments`, real wall-clock injection, reference data,
//! and order construction.

mod support;

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

use bolt_v2::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3InstrumentFilterClockFn,
        map_bolt_v3_adapters_with_instrument_filters,
    },
    bolt_v3_config::{LoadedStrategy, load_bolt_v3_config},
    bolt_v3_instrument_filters::InstrumentFilterConfig,
    bolt_v3_market_families::instrument_filters_from_config,
    bolt_v3_providers::{
        binance::ResolvedBoltV3BinanceSecrets, polymarket::ResolvedBoltV3PolymarketSecrets,
    },
    bolt_v3_secrets::{ResolvedBoltV3Secrets, ResolvedBoltV3VenueSecrets},
};
use nautilus_polymarket::config::PolymarketDataClientConfig;

/// Mutate a single field in the strategy's raw `[target]` TOML
/// envelope. Mirrors the helper in `tests/bolt_v3_instrument_filters.rs`;
/// the strategy envelope keeps `target` as a generic raw-TOML
/// container so market-family-shaped fields live in the per-family
/// binding module.
fn set_target_field(strategy: &mut LoadedStrategy, key: &str, value: toml::Value) {
    strategy
        .config
        .target
        .as_table_mut()
        .expect("strategy [target] should be a TOML table")
        .insert(key.to_string(), value);
}

fn fixture_resolved_secrets() -> ResolvedBoltV3Secrets {
    let mut venues: BTreeMap<String, ResolvedBoltV3VenueSecrets> = BTreeMap::new();
    venues.insert(
        "polymarket_main".to_string(),
        Arc::new(ResolvedBoltV3PolymarketSecrets {
            private_key: "binding-poly-private-key".to_string(),
            api_key: "binding-poly-api-key".to_string(),
            api_secret: "binding-poly-api-secret".to_string(),
            passphrase: "binding-poly-passphrase".to_string(),
        }),
    );
    venues.insert(
        "binance_reference".to_string(),
        Arc::new(ResolvedBoltV3BinanceSecrets {
            api_key: "binding-binance-api-key".to_string(),
            api_secret: "binding-binance-api-secret".to_string(),
        }),
    );
    ResolvedBoltV3Secrets { venues }
}

fn fixed_clock(now_unix_seconds: i64) -> BoltV3InstrumentFilterClockFn {
    Arc::new(move || now_unix_seconds)
}

#[test]
fn provider_binding_installs_polymarket_filter_for_updown_target_at_fixed_time() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();
    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("instrument filters should derive cleanly");

    // Fixed `now_unix_seconds = 601` puts the clock inside the
    // BTC/5m window [600, 900): current=600 and next=900. The provider
    // binding's filter must surface those slugs in `[current, next]`
    // order on every `market_slugs()` call.
    let clock = fixed_clock(601);

    let configs = map_bolt_v3_adapters_with_instrument_filters(
        &loaded,
        &resolved,
        &instrument_filters,
        clock,
    )
    .expect("mapping with instrument filter should succeed");

    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present in mapper output");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");

    assert_eq!(
        data.filters.len(),
        1,
        "exactly one provider filter should be installed for the single updown target"
    );
    assert_eq!(
        data.auto_load_debounce_ms, 250,
        "provider binding must take the NT auto-load debounce from TOML, not from a code literal"
    );
    assert!(
        !data.auto_load_missing_instruments,
        "fixture keeps NT missing-instrument auto-load disabled through TOML"
    );
    let slugs = data.filters[0]
        .market_slugs()
        .expect("provider filter must yield Some(slugs) when bound to an updown target");
    assert_eq!(
        slugs,
        vec![
            "btc-updown-5m-600".to_string(),
            "btc-updown-5m-900".to_string(),
        ],
        "provider filter slug ordering must be [current, next]"
    );
}

#[test]
fn provider_binding_forwards_auto_load_missing_instruments_from_toml() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let data = loaded
        .root
        .venues
        .get_mut("polymarket_main")
        .and_then(|venue| venue.data.as_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture should expose polymarket data table");
    data.insert(
        "auto_load_missing_instruments".to_string(),
        toml::Value::Boolean(true),
    );

    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("instrument filters should build");
    let resolved = fixture_resolved_secrets();
    let configs = map_bolt_v3_adapters_with_instrument_filters(
        &loaded,
        &resolved,
        &instrument_filters,
        fixed_clock(601),
    )
    .expect("mapping with instrument filter should succeed");

    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present in mapper output");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");

    assert!(
        data.auto_load_missing_instruments,
        "provider binding must take NT missing-instrument auto-load from TOML"
    );
}

#[test]
fn provider_binding_preserves_declaration_order_across_multiple_updown_targets() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Build three strategies whose declaration sequence is deliberately
    // NON-MONOTONIC across every likely accidental sort key
    // (strategy_instance_id, configured_target_id, underlying_asset,
    // cadence_seconds, cadence_slug_token). Any accidental `sort_by`
    // inside the binding layer would re-order at least one index and
    // trip a per-index slug assertion below.
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

    let resolved = fixture_resolved_secrets();
    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("instrument filters should derive cleanly");

    // Pick now=7300:
    //   15m cadence 900  -> floor(7300/900)*900 = 7200, next = 8100
    //   5m  cadence 300  -> floor(7300/300)*300 = 7200, next = 7500
    //   1h  cadence 3600 -> floor(7300/3600)*3600 = 7200, next = 10800
    let clock = fixed_clock(7300);

    let configs = map_bolt_v3_adapters_with_instrument_filters(
        &loaded,
        &resolved,
        &instrument_filters,
        clock,
    )
    .expect("mapping should succeed");

    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");

    assert_eq!(
        data.filters.len(),
        3,
        "three updown targets must produce three provider filters"
    );

    assert_eq!(
        data.filters[0].market_slugs(),
        Some(vec![
            "ltc-updown-15m-7200".to_string(),
            "ltc-updown-15m-8100".to_string(),
        ]),
        "filters[0] must correspond to declared strategy [0] (zeta/LTC/15m)"
    );
    assert_eq!(
        data.filters[1].market_slugs(),
        Some(vec![
            "xrp-updown-5m-7200".to_string(),
            "xrp-updown-5m-7500".to_string(),
        ]),
        "filters[1] must correspond to declared strategy [1] (alpha/XRP/5m)"
    );
    assert_eq!(
        data.filters[2].market_slugs(),
        Some(vec![
            "btc-updown-1h-7200".to_string(),
            "btc-updown-1h-10800".to_string(),
        ]),
        "filters[2] must correspond to declared strategy [2] (mike/BTC/1h)"
    );
}

#[test]
fn instrument_filters_path_forwards_configured_subscribe_new_markets() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    let polymarket_data = loaded
        .root
        .venues
        .get_mut("polymarket_main")
        .and_then(|venue| venue.data.as_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture polymarket data table should exist");
    polymarket_data.insert(
        "subscribe_new_markets".to_string(),
        toml::Value::Boolean(true),
    );

    let resolved = fixture_resolved_secrets();
    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("instrument filters should derive cleanly");
    let clock = fixed_clock(0);

    let configs = map_bolt_v3_adapters_with_instrument_filters(
        &loaded,
        &resolved,
        &instrument_filters,
        clock,
    )
    .expect("configured subscribe_new_markets should map through instrument-filter path");
    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present in mapper output");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must map")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket [data] should downcast to NT PolymarketDataClientConfig");
    assert!(data.subscribe_new_markets);
}

#[test]
fn empty_instrument_filter_config_installs_no_provider_filter() {
    // A configuration with no rotating-market targets must produce no
    // provider filter installation. This pins down the "filter only
    // when a configured instrument-filter target exists" half of the binding contract;
    // accidentally always-installing a filter would otherwise be
    // invisible to the single-target test above.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();

    let empty_instrument_filter_config = InstrumentFilterConfig::empty();
    let clock = fixed_clock(0);

    let configs = map_bolt_v3_adapters_with_instrument_filters(
        &loaded,
        &resolved,
        &empty_instrument_filter_config,
        clock,
    )
    .expect("mapping should succeed");

    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");
    assert!(
        data.filters.is_empty(),
        "an empty InstrumentFilterConfig must not install any provider filter"
    );
    assert!(
        data.new_market_filter.is_none(),
        "no `new_market_filter` should be smuggled in via the binding layer"
    );
}

#[test]
fn polymarket_filter_binding_uses_supported_market_family_registry() {
    let source = include_str!("../src/bolt_v3_providers/polymarket.rs");
    assert!(
        source.contains("SUPPORTED_MARKET_FAMILIES.contains(&target.family_key)"),
        "Polymarket filter binding must derive accepted target families from SUPPORTED_MARKET_FAMILIES"
    );
    assert!(
        !source.contains(".filter(|target| target.family_key == updown::KEY)"),
        "Polymarket filter binding must not repeat a concrete family key outside the supported-family registry"
    );
}

#[test]
fn provider_binding_filter_recomputes_slug_pair_each_call_against_advancing_clock() {
    // The pinned NT contract for `MarketSlugFilter::new` re-evaluates
    // the closure on every `load_all` cycle so the slug pair rolls
    // forward with cadence. This test pins that dynamic re-evaluation
    // by injecting an `AtomicI64`-backed clock, advancing it by one
    // full cadence between two `market_slugs()` calls, and asserting
    // the filter surfaces the rolled-forward `[current, next]` pair.
    // A future regression that wraps the closure result in a `OnceCell`
    // (or otherwise memoises the slug list) would fail the second
    // assertion below.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();
    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("instrument filters should derive cleanly");

    let counter = Arc::new(AtomicI64::new(601));
    let clock_handle = counter.clone();
    let clock: BoltV3InstrumentFilterClockFn =
        Arc::new(move || clock_handle.load(Ordering::Relaxed));

    let configs = map_bolt_v3_adapters_with_instrument_filters(
        &loaded,
        &resolved,
        &instrument_filters,
        clock,
    )
    .expect("mapping should succeed");

    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");
    let filter = &data.filters[0];

    // First cycle at counter=601: BTC/5m window [600, 900).
    assert_eq!(
        filter.market_slugs(),
        Some(vec![
            "btc-updown-5m-600".to_string(),
            "btc-updown-5m-900".to_string(),
        ]),
        "first market_slugs() call must reflect counter=601"
    );

    // Advance the clock by one full cadence; the filter MUST recompute.
    counter.store(901, Ordering::Relaxed);

    assert_eq!(
        filter.market_slugs(),
        Some(vec![
            "btc-updown-5m-900".to_string(),
            "btc-updown-5m-1200".to_string(),
        ]),
        "second market_slugs() call must reflect counter=901; \
         caching the slug list would fail this assertion"
    );
}

#[test]
fn provider_binding_filter_returns_empty_market_slugs_when_period_pair_overflows() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();
    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("instrument filters should derive cleanly");
    let clock = fixed_clock(i64::MAX);

    let configs = map_bolt_v3_adapters_with_instrument_filters(
        &loaded,
        &resolved,
        &instrument_filters,
        clock,
    )
    .expect("mapping should still succeed; the filter must return market slugs per cycle");

    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");

    assert_eq!(
        data.filters[0].market_slugs(),
        Some(Vec::new()),
        "period-pair overflow must produce an empty market_slugs result"
    );
}

#[test]
fn provider_binding_rejects_updown_target_bound_to_non_polymarket_venue() {
    // The binding layer must fail loud if a configured rotating-market
    // target points at a non-Polymarket venue. Without this guard the
    // target would be silently dropped, because filter installation
    // only runs on the Polymarket branch of the venue iteration.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Mutate the strategy to bind to the Binance reference venue.
    loaded.strategies[0].config.venue = "binance_reference".to_string();

    let resolved = fixture_resolved_secrets();
    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("instrument filters should derive cleanly");
    let clock = fixed_clock(0);

    let error = map_bolt_v3_adapters_with_instrument_filters(
        &loaded,
        &resolved,
        &instrument_filters,
        clock,
    )
    .expect_err("non-polymarket venue binding must fail loud at the adapter boundary");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "binance_reference");
            assert_eq!(field, "strategy.venue");
            assert!(
                message.contains("does not support that market family"),
                "error message should explain the family/provider compatibility boundary: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn provider_binding_rejects_updown_target_bound_to_unknown_venue() {
    // A target whose strategy venue does not appear under `[venues]`
    // is also a misconfiguration the binding layer must reject
    // explicitly rather than silently produce no filter.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    loaded.strategies[0].config.venue = "venue_does_not_exist".to_string();

    let resolved = fixture_resolved_secrets();
    let instrument_filters =
        instrument_filters_from_config(&loaded).expect("instrument filters should derive cleanly");
    let clock = fixed_clock(0);

    let error = map_bolt_v3_adapters_with_instrument_filters(
        &loaded,
        &resolved,
        &instrument_filters,
        clock,
    )
    .expect_err("unknown venue binding must fail loud at the adapter boundary");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "venue_does_not_exist");
            assert_eq!(field, "strategy.venue");
            assert!(
                message.contains("unknown venue"),
                "error message should describe the unknown-venue case: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn strategy_registry_does_not_import_polymarket_fee_provider() {
    let source = include_str!("../src/strategies/registry.rs");
    for forbidden in ["clients::polymarket", "PolymarketClobFeeProvider"] {
        assert!(
            !source.contains(forbidden),
            "src/strategies/registry.rs must expose a generic fee-provider trait without importing a concrete provider; found `{forbidden}`"
        );
    }
}

#[test]
fn provider_binding_root_does_not_import_polymarket_fee_provider_client() {
    let source = include_str!("../src/bolt_v3_providers/mod.rs");
    for forbidden in ["clients::polymarket", "PolymarketClobFeeProvider"] {
        assert!(
            !source.contains(forbidden),
            "src/bolt_v3_providers/mod.rs may expose provider bindings but must not import concrete Polymarket fee clients; found `{forbidden}`"
        );
    }
}

#[test]
fn polymarket_provider_binding_does_not_import_legacy_modules() {
    let source = include_str!("../src/bolt_v3_providers/polymarket.rs");
    for forbidden in [
        "clients::polymarket",
        "crate::secrets",
        "secrets::pad_base64",
    ] {
        assert!(
            !source.contains(forbidden),
            "src/bolt_v3_providers/polymarket.rs must keep fee and secret helpers inside the bolt-v3 provider binding; found `{forbidden}`"
        );
    }
}

#[test]
fn polymarket_fee_provider_module_does_not_import_root_secret_helpers() {
    let source = include_str!("../src/bolt_v3_providers/polymarket/fees.rs");
    for forbidden in [
        "clients::polymarket",
        "crate::secrets",
        "secrets::pad_base64",
        "PolymarketSecrets",
    ] {
        assert!(
            !source.contains(forbidden),
            "src/bolt_v3_providers/polymarket/fees.rs must not import root secret helpers; found `{forbidden}`"
        );
    }
}

#[test]
fn binary_oracle_archetype_does_not_name_concrete_fee_provider() {
    let source = include_str!("../src/bolt_v3_archetypes/binary_oracle_edge_taker.rs");
    for forbidden in [
        "bolt_v3_providers::polymarket",
        "polymarket::KEY",
        "polymarket::build_fee_provider",
        "PolymarketClobFeeProvider",
    ] {
        assert!(
            !source.contains(forbidden),
            "binary_oracle_edge_taker archetype must request fee providers through the provider binding surface; found `{forbidden}`"
        );
    }
}
