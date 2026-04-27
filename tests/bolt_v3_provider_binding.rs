//! Provider-binding tests for bolt-v3.
//!
//! These tests guard the product boundary that was articulated after
//! Slice 9: core market-identity in `bolt_v3_market_identity` is
//! provider-neutral, and translation of that neutral plan into
//! provider-shaped NT adapter values (today: a `MarketSlugFilter`
//! installed on `PolymarketDataClientConfig.filters`) is the sole
//! responsibility of the adapter / provider-binding layer.
//!
//! What these tests prove:
//!   1. The new market-identity-aware mapper installs exactly one
//!      provider filter per configured updown target on the matching
//!      venue, and the filter yields `[current_slug, next_slug]` for
//!      the injected fixed clock.
//!   2. Multi-target filter ordering follows declared strategy
//!      sequence and never reorders by an accidental sort key.
//!   3. The `subscribe_new_markets = true` validation invariant still
//!      fires through the market-identity entry point so the binding
//!      layer cannot be used to smuggle an "all markets" subscription.
//!   4. An empty market-identity plan installs no provider filter,
//!      preserving the previous default behaviour for non-rotating
//!      configurations.
//!
//! Out of scope: live `LiveNode` runtime, NT cache reads,
//! `request_instruments`, real wall-clock injection, dynamic market
//! selection, fused / reference price derivation, and any trade-action
//! construction. Those boundaries belong to later slices.

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
        BoltV3AdapterMappingError, BoltV3UpdownNowFn, BoltV3VenueAdapterConfig,
        map_bolt_v3_adapters_with_market_identity,
    },
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_market_identity::{MarketIdentityPlan, plan_market_identity},
    bolt_v3_secrets::{
        ResolvedBoltV3BinanceSecrets, ResolvedBoltV3PolymarketSecrets, ResolvedBoltV3Secrets,
        ResolvedBoltV3VenueSecrets,
    },
};

fn fixture_resolved_secrets() -> ResolvedBoltV3Secrets {
    let mut venues = BTreeMap::new();
    venues.insert(
        "polymarket_main".to_string(),
        ResolvedBoltV3VenueSecrets::Polymarket(ResolvedBoltV3PolymarketSecrets {
            private_key: "binding-poly-private-key".to_string(),
            api_key: "binding-poly-api-key".to_string(),
            api_secret: "binding-poly-api-secret".to_string(),
            passphrase: "binding-poly-passphrase".to_string(),
        }),
    );
    venues.insert(
        "binance_reference".to_string(),
        ResolvedBoltV3VenueSecrets::Binance(ResolvedBoltV3BinanceSecrets {
            api_key: "binding-binance-api-key".to_string(),
            api_secret: "binding-binance-api-secret".to_string(),
        }),
    );
    ResolvedBoltV3Secrets { venues }
}

fn fixed_clock(now_unix_seconds: i64) -> BoltV3UpdownNowFn {
    Arc::new(move || now_unix_seconds)
}

#[test]
fn provider_binding_installs_polymarket_filter_for_updown_target_at_fixed_time() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");

    // Fixed `now_unix_seconds = 601` puts the planner inside the
    // BTC/5m window [600, 900): current=600 and next=900. The provider
    // binding's filter must surface those slugs in `[current, next]`
    // order on every `market_slugs()` call.
    let clock = fixed_clock(601);

    let configs = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect("mapping with market identity should succeed");

    let polymarket = match configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present in mapper output")
    {
        BoltV3VenueAdapterConfig::Polymarket(inner) => inner,
        BoltV3VenueAdapterConfig::Binance(_) => {
            panic!("polymarket_main must map to a Polymarket adapter config")
        }
    };
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config");

    assert_eq!(
        data.filters.len(),
        1,
        "exactly one provider filter should be installed for the single updown target"
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
        first.config.target.configured_target_id = "ltc_updown_15m".to_string();
        first.config.target.underlying_asset = "LTC".to_string();
        first.config.target.cadence_seconds = 900;
    }
    second.config.strategy_instance_id = "alpha_strategy_main".to_string();
    second.config.target.configured_target_id = "xrp_updown_5m".to_string();
    second.config.target.underlying_asset = "XRP".to_string();
    second.config.target.cadence_seconds = 300;

    third.config.strategy_instance_id = "mike_strategy_main".to_string();
    third.config.target.configured_target_id = "btc_updown_1h".to_string();
    third.config.target.underlying_asset = "BTC".to_string();
    third.config.target.cadence_seconds = 3600;

    loaded.strategies.push(second);
    loaded.strategies.push(third);

    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");

    // Pick now=7300:
    //   15m cadence 900  -> floor(7300/900)*900 = 7200, next = 8100
    //   5m  cadence 300  -> floor(7300/300)*300 = 7200, next = 7500
    //   1h  cadence 3600 -> floor(7300/3600)*3600 = 7200, next = 10800
    let clock = fixed_clock(7300);

    let configs = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect("mapping should succeed");

    let polymarket = match configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present")
    {
        BoltV3VenueAdapterConfig::Polymarket(inner) => inner,
        BoltV3VenueAdapterConfig::Binance(_) => {
            panic!("polymarket_main must map to a Polymarket adapter config")
        }
    };
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config");

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
fn market_identity_path_still_rejects_subscribe_new_markets_true() {
    // The previous mapper boundary refused to forward
    // `subscribe_new_markets = true` to NT (which would otherwise cause
    // pinned NT to subscribe to every Polymarket market). The new
    // market-identity-aware entry point must preserve that invariant
    // so the provider-binding layer cannot be used to smuggle a broad
    // subscription path under the cover of "we have a filter now".
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
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");
    let clock = fixed_clock(0);

    let error = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect_err("mapper must not forward subscribe_new_markets=true to NT");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key, field, ..
        } => {
            assert_eq!(venue_key, "polymarket_main");
            assert_eq!(field, "data.subscribe_new_markets");
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn empty_market_identity_plan_installs_no_provider_filter() {
    // A configuration with no rotating-market targets must produce no
    // provider filter installation. This pins down the "filter only
    // when configured identity exists" half of the binding contract;
    // accidentally always-installing a filter would otherwise be
    // invisible to the single-target test above.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();

    let empty_plan = MarketIdentityPlan {
        updown_targets: Vec::new(),
    };
    let clock = fixed_clock(0);

    let configs = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &empty_plan, clock)
        .expect("mapping should succeed");

    let polymarket = match configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present")
    {
        BoltV3VenueAdapterConfig::Polymarket(inner) => inner,
        BoltV3VenueAdapterConfig::Binance(_) => {
            panic!("polymarket_main must map to a Polymarket adapter config")
        }
    };
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config");
    assert!(
        data.filters.is_empty(),
        "an empty market-identity plan must not install any provider filter"
    );
    assert!(
        data.new_market_filter.is_none(),
        "no `new_market_filter` should be smuggled in via the binding layer"
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
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");

    let counter = Arc::new(AtomicI64::new(601));
    let clock_handle = counter.clone();
    let clock: BoltV3UpdownNowFn = Arc::new(move || clock_handle.load(Ordering::Relaxed));

    let configs = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect("mapping should succeed");

    let polymarket = match configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present")
    {
        BoltV3VenueAdapterConfig::Polymarket(inner) => inner,
        BoltV3VenueAdapterConfig::Binance(_) => {
            panic!("polymarket_main must map to a Polymarket adapter config")
        }
    };
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config");
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
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");
    let clock = fixed_clock(0);

    let error = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect_err("non-polymarket venue binding must fail loud at the adapter boundary");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "binance_reference");
            assert_eq!(field, "strategy.target.venue_config_key");
            assert!(
                message.contains("polymarket"),
                "error message should explain the required venue kind: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn provider_binding_rejects_updown_target_bound_to_unknown_venue() {
    // A target whose `venue_config_key` does not appear under
    // `[venues]` is also a misconfiguration the binding layer must
    // reject explicitly rather than silently produce no filter.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    loaded.strategies[0].config.venue = "venue_does_not_exist".to_string();

    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");
    let clock = fixed_clock(0);

    let error = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect_err("unknown venue binding must fail loud at the adapter boundary");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "venue_does_not_exist");
            assert_eq!(field, "strategy.target.venue_config_key");
            assert!(
                message.contains("unknown venue"),
                "error message should describe the unknown-venue case: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}
