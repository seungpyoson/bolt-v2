mod support;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_market_families::updown::{
        BoltV3MarketIdentityError, MarketIdentityPlan, UpdownTargetPlan, plan_market_identity,
    },
    bolt_v3_provider_family_bindings::polymarket_updown::{
        UpdownSelectedMarketFailureReason, UpdownSelectedMarketResolution,
        UpdownSelectedMarketRole, resolve_updown_selected_market_from_cache,
        resolve_updown_selected_markets_for_client_from_cache,
    },
};
use nautilus_common::cache::Cache;
use nautilus_core::{Params, UnixNanos};
use nautilus_model::{
    enums::AssetClass,
    identifiers::InstrumentId,
    instruments::{InstrumentAny, binary_option::BinaryOption},
    types::{Currency, Price, Quantity},
};
use nautilus_polymarket::common::consts::POLYMARKET_VENUE;
use serde_json::json;
use support::UpdownSelectedMarketReadinessRole;
use ustr::Ustr;

fn target_plan() -> UpdownTargetPlan {
    existing_strategy_plan().updown_targets[0].clone()
}

fn existing_strategy_plan() -> MarketIdentityPlan {
    let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root_multi.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("multi-strategy fixture should load");
    plan_market_identity(&loaded).expect("market identity plan should build")
}

fn polymarket_updown_option(
    market: &support::UpdownSelectedMarketFixture,
    leg: &support::UpdownSelectedMarketLegFixture,
) -> InstrumentAny {
    let instrument_id = InstrumentId::from(leg.instrument_id.as_str());
    let price_increment = Price::from("0.001");
    let size_increment = Quantity::from("0.01");
    let mut info = Params::new();
    info.insert("token_id".to_string(), json!(leg.token_id));
    info.insert("condition_id".to_string(), json!(market.condition_id));
    info.insert("question_id".to_string(), json!(market.question_id));
    info.insert("market_slug".to_string(), json!(market.market_slug));

    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        instrument_id.symbol,
        AssetClass::Alternative,
        Currency::USDC(),
        UnixNanos::from(market.start_ms * 1_000_000),
        UnixNanos::from(market.end_ms * 1_000_000),
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        Some(Ustr::from(leg.outcome.as_str())),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(info),
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

fn add_fixture_market(cache: &mut Cache, market: &support::UpdownSelectedMarketFixture) {
    for leg in &market.legs {
        cache
            .add_instrument(polymarket_updown_option(market, leg))
            .unwrap();
    }
}

#[test]
fn cached_current_updown_pair_resolves_selected_market_identity() {
    let target = target_plan();
    let venue = *POLYMARKET_VENUE;
    let mut cache = Cache::new(None, None);
    let current_market = support::bolt_v3_updown_readiness_selected_market_fixture(
        UpdownSelectedMarketReadinessRole::Current,
    );
    add_fixture_market(&mut cache, &current_market);

    let resolution = resolve_updown_selected_market_from_cache(&cache, &target, &venue, 601_000)
        .expect("selection should not fail on identity math");

    match resolution {
        UpdownSelectedMarketResolution::Selected {
            role,
            selected_market,
        } => {
            let up_leg = current_market.leg("Up");
            let down_leg = current_market.leg("Down");
            assert_eq!(role, UpdownSelectedMarketRole::Current);
            assert_eq!(
                selected_market.market_selection_type,
                target.market_selection_type
            );
            assert_eq!(selected_market.client_id, target.client_id_key);
            assert_eq!(selected_market.venue, venue.as_str());
            assert_eq!(selected_market.rotating_market_family, "updown");
            assert_eq!(
                selected_market.polymarket_condition_id,
                current_market.condition_id
            );
            assert_eq!(
                selected_market.polymarket_market_slug,
                current_market.market_slug
            );
            assert_eq!(
                selected_market.polymarket_question_id,
                current_market.question_id
            );
            assert_eq!(selected_market.up_instrument_id, up_leg.instrument_id);
            assert_eq!(selected_market.down_instrument_id, down_leg.instrument_id);
            assert_eq!(
                selected_market.polymarket_market_start_timestamp_milliseconds,
                i64::try_from(current_market.start_ms).unwrap()
            );
            assert_eq!(
                selected_market.polymarket_market_end_timestamp_milliseconds,
                i64::try_from(current_market.end_ms).unwrap()
            );
        }
        other => panic!("expected selected current market; got {other:?}"),
    }
}

#[test]
fn current_slug_with_non_current_time_window_does_not_select_market() {
    let target = target_plan();
    let venue = *POLYMARKET_VENUE;
    let mut cache = Cache::new(None, None);
    let stale_market = support::bolt_v3_updown_readiness_selected_market_fixture(
        UpdownSelectedMarketReadinessRole::Stale,
    );
    add_fixture_market(&mut cache, &stale_market);

    let resolution = resolve_updown_selected_market_from_cache(&cache, &target, &venue, 601_000)
        .expect("selection should not fail on identity math");

    assert_eq!(
        resolution,
        UpdownSelectedMarketResolution::Failed {
            failure_reason: UpdownSelectedMarketFailureReason::NoSelectedMarket
        }
    );
}

#[test]
fn negative_market_selection_timestamp_is_rejected() {
    let target = target_plan();
    let venue = *POLYMARKET_VENUE;
    let cache = Cache::new(None, None);

    let error = resolve_updown_selected_market_from_cache(&cache, &target, &venue, -1)
        .expect_err("negative selection timestamp must fail before cache lookup");

    assert!(matches!(
        error,
        BoltV3MarketIdentityError::NegativeNowUnixSeconds {
            now_unix_seconds: -1
        }
    ));
}

#[test]
fn multiple_current_updown_pairs_for_same_target_fail_ambiguous() {
    let target = target_plan();
    let venue = *POLYMARKET_VENUE;
    let mut cache = Cache::new(None, None);
    for market in [
        support::bolt_v3_updown_readiness_selected_market_fixture(
            UpdownSelectedMarketReadinessRole::AmbiguousA,
        ),
        support::bolt_v3_updown_readiness_selected_market_fixture(
            UpdownSelectedMarketReadinessRole::AmbiguousB,
        ),
    ] {
        add_fixture_market(&mut cache, &market);
    }

    let resolution = resolve_updown_selected_market_from_cache(&cache, &target, &venue, 601_000)
        .expect("selection should not fail on identity math");

    assert_eq!(
        resolution,
        UpdownSelectedMarketResolution::Failed {
            failure_reason: UpdownSelectedMarketFailureReason::AmbiguousSelectedMarket
        }
    );
}

#[test]
fn config_path_resolves_each_client_target_from_nt_cache() {
    let plan = existing_strategy_plan();
    let first_target = &plan.updown_targets[0];
    let second_target = &plan.updown_targets[1];
    let client_id = plan.updown_targets[0].client_id_key.clone();
    let venue = *POLYMARKET_VENUE;
    let mut cache = Cache::new(None, None);
    let current_market = support::bolt_v3_updown_readiness_selected_market_fixture(
        UpdownSelectedMarketReadinessRole::Current,
    );
    add_fixture_market(&mut cache, &current_market);

    let resolutions = resolve_updown_selected_markets_for_client_from_cache(
        &cache, &plan, &client_id, &venue, 601_000,
    )
    .expect("cache resolution should not fail on identity math");

    assert_eq!(resolutions.len(), 2);
    assert_eq!(
        resolutions[0].strategy_instance_id.as_str(),
        first_target.strategy_instance_id.as_str()
    );
    assert_eq!(
        resolutions[0].configured_target_id.as_str(),
        first_target.configured_target_id.as_str()
    );
    assert!(matches!(
        resolutions[0].resolution,
        UpdownSelectedMarketResolution::Selected {
            role: UpdownSelectedMarketRole::Current,
            ..
        }
    ));
    assert_eq!(
        resolutions[1].strategy_instance_id.as_str(),
        second_target.strategy_instance_id.as_str()
    );
    assert_eq!(
        resolutions[1].configured_target_id.as_str(),
        second_target.configured_target_id.as_str()
    );
    assert_eq!(
        resolutions[1].resolution,
        UpdownSelectedMarketResolution::Failed {
            failure_reason: UpdownSelectedMarketFailureReason::InstrumentsNotInCache
        }
    );
}
