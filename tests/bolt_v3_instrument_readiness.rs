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
    instrument_id: &str,
    token_id: &str,
    condition_id: &str,
    question_id: &str,
    market_slug: &str,
    outcome: &str,
    start_ms: u64,
    end_ms: u64,
) -> InstrumentAny {
    let instrument_id = InstrumentId::from(instrument_id);
    let price_increment = Price::from("0.001");
    let size_increment = Quantity::from("0.01");
    let mut info = Params::new();
    info.insert("token_id".to_string(), json!(token_id));
    info.insert("condition_id".to_string(), json!(condition_id));
    info.insert("question_id".to_string(), json!(question_id));
    info.insert("market_slug".to_string(), json!(market_slug));

    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        instrument_id.symbol,
        AssetClass::Alternative,
        Currency::USDC(),
        UnixNanos::from(start_ms * 1_000_000),
        UnixNanos::from(end_ms * 1_000_000),
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        Some(Ustr::from(outcome)),
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

#[test]
fn cached_current_updown_pair_resolves_selected_market_identity() {
    let target = target_plan();
    let venue = *POLYMARKET_VENUE;
    let mut cache = Cache::new(None, None);
    cache
        .add_instrument(polymarket_updown_option(
            "0xcurrent-111.POLYMARKET",
            "111",
            "0xcurrent",
            "question-current",
            "eth-updown-5m-600",
            "Up",
            600_000,
            900_000,
        ))
        .unwrap();
    cache
        .add_instrument(polymarket_updown_option(
            "0xcurrent-222.POLYMARKET",
            "222",
            "0xcurrent",
            "question-current",
            "eth-updown-5m-600",
            "Down",
            600_000,
            900_000,
        ))
        .unwrap();

    let resolution = resolve_updown_selected_market_from_cache(&cache, &target, &venue, 601_000)
        .expect("selection should not fail on identity math");

    match resolution {
        UpdownSelectedMarketResolution::Selected {
            role,
            selected_market,
        } => {
            assert_eq!(role, UpdownSelectedMarketRole::Current);
            assert_eq!(selected_market.market_selection_type, "rotating_market");
            assert_eq!(selected_market.client_id, "polymarket_main");
            assert_eq!(selected_market.venue, "POLYMARKET");
            assert_eq!(selected_market.rotating_market_family, "updown");
            assert_eq!(selected_market.polymarket_condition_id, "0xcurrent");
            assert_eq!(selected_market.polymarket_market_slug, "eth-updown-5m-600");
            assert_eq!(selected_market.polymarket_question_id, "question-current");
            assert_eq!(selected_market.up_instrument_id, "0xcurrent-111.POLYMARKET");
            assert_eq!(
                selected_market.down_instrument_id,
                "0xcurrent-222.POLYMARKET"
            );
            assert_eq!(
                selected_market.polymarket_market_start_timestamp_milliseconds,
                600_000
            );
            assert_eq!(
                selected_market.polymarket_market_end_timestamp_milliseconds,
                900_000
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
    cache
        .add_instrument(polymarket_updown_option(
            "0xstale-111.POLYMARKET",
            "111",
            "0xstale",
            "question-stale",
            "eth-updown-5m-600",
            "Up",
            300_000,
            600_000,
        ))
        .unwrap();
    cache
        .add_instrument(polymarket_updown_option(
            "0xstale-222.POLYMARKET",
            "222",
            "0xstale",
            "question-stale",
            "eth-updown-5m-600",
            "Down",
            300_000,
            600_000,
        ))
        .unwrap();

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
    for (condition_id, up_token, down_token) in [
        ("0xambiguous-a", "111", "222"),
        ("0xambiguous-b", "333", "444"),
    ] {
        cache
            .add_instrument(polymarket_updown_option(
                &format!("{condition_id}-{up_token}.POLYMARKET"),
                up_token,
                condition_id,
                "question-ambiguous",
                "eth-updown-5m-600",
                "Up",
                600_000,
                900_000,
            ))
            .unwrap();
        cache
            .add_instrument(polymarket_updown_option(
                &format!("{condition_id}-{down_token}.POLYMARKET"),
                down_token,
                condition_id,
                "question-ambiguous",
                "eth-updown-5m-600",
                "Down",
                600_000,
                900_000,
            ))
            .unwrap();
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
    let client_id = plan.updown_targets[0].client_id_key.clone();
    let venue = *POLYMARKET_VENUE;
    let mut cache = Cache::new(None, None);
    cache
        .add_instrument(polymarket_updown_option(
            "0xcurrent-111.POLYMARKET",
            "111",
            "0xcurrent",
            "question-current",
            "eth-updown-5m-600",
            "Up",
            600_000,
            900_000,
        ))
        .unwrap();
    cache
        .add_instrument(polymarket_updown_option(
            "0xcurrent-222.POLYMARKET",
            "222",
            "0xcurrent",
            "question-current",
            "eth-updown-5m-600",
            "Down",
            600_000,
            900_000,
        ))
        .unwrap();

    let resolutions = resolve_updown_selected_markets_for_client_from_cache(
        &cache, &plan, &client_id, &venue, 601_000,
    )
    .expect("cache resolution should not fail on identity math");

    assert_eq!(resolutions.len(), 2);
    assert_eq!(
        resolutions[0].strategy_instance_id,
        "ETHCHAINLINKTAKER-V3-001"
    );
    assert_eq!(resolutions[0].configured_target_id, "eth_updown_5m");
    assert!(matches!(
        resolutions[0].resolution,
        UpdownSelectedMarketResolution::Selected {
            role: UpdownSelectedMarketRole::Current,
            ..
        }
    ));
    assert_eq!(
        resolutions[1].strategy_instance_id,
        "ETHCHAINLINKTAKER-V3-015"
    );
    assert_eq!(resolutions[1].configured_target_id, "eth_updown_15m");
    assert_eq!(
        resolutions[1].resolution,
        UpdownSelectedMarketResolution::Failed {
            failure_reason: UpdownSelectedMarketFailureReason::InstrumentsNotInCache
        }
    );
}
