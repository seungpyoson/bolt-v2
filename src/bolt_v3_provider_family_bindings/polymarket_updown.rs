use std::{collections::BTreeMap, sync::Arc};

use nautilus_common::cache::Cache;
use nautilus_model::{
    identifiers::Venue,
    instruments::{InstrumentAny, binary_option::BinaryOption},
};
use nautilus_polymarket::filters::{InstrumentFilter, MarketSlugFilter};

use crate::{
    bolt_v3_adapters::BoltV3MarketSelectionNowFn,
    bolt_v3_market_families::updown::{
        self, BoltV3MarketIdentityError, MarketIdentityPlan, UpdownTargetPlan,
        candidates_for_target, updown_market_slug, updown_period_pair,
    },
    bolt_v3_provider_family_bindings::{ProviderFamilyFilterContext, ProviderMarketFamilyBinding},
    bolt_v3_providers::{
        ProviderInstrumentReadinessContext, ProviderInstrumentReadinessFact,
        ProviderInstrumentReadinessStatus, polymarket,
    },
};

pub const BINDING: ProviderMarketFamilyBinding = ProviderMarketFamilyBinding {
    provider_key: polymarket::KEY,
    family_key: updown::KEY,
    build_polymarket_filters: Some(build_market_slug_filters_for_client),
    check_instrument_readiness: Some(check_instrument_readiness),
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdownSelectedMarketRole {
    Current,
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdownSelectedMarketFailureReason {
    InstrumentsNotInCache,
    NoSelectedMarket,
    AmbiguousSelectedMarket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownSelectedMarket {
    pub market_selection_type: String,
    pub client_id: String,
    pub venue: String,
    pub rotating_market_family: String,
    pub polymarket_condition_id: String,
    pub polymarket_market_slug: String,
    pub polymarket_question_id: String,
    pub up_instrument_id: String,
    pub down_instrument_id: String,
    pub polymarket_market_start_timestamp_milliseconds: i64,
    pub polymarket_market_end_timestamp_milliseconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdownSelectedMarketResolution {
    Selected {
        role: UpdownSelectedMarketRole,
        selected_market: UpdownSelectedMarket,
    },
    Failed {
        failure_reason: UpdownSelectedMarketFailureReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownTargetSelectedMarketResolution {
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub resolution: UpdownSelectedMarketResolution,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct UpdownMarketKey {
    condition_id: String,
    market_slug: String,
    question_id: String,
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug, Clone, Default)]
struct UpdownInstrumentPair {
    up_instrument_id: Option<String>,
    down_instrument_id: Option<String>,
}

pub fn resolve_updown_selected_market_from_cache(
    cache: &Cache,
    target: &UpdownTargetPlan,
    venue: &Venue,
    market_selection_timestamp_milliseconds: i64,
) -> Result<UpdownSelectedMarketResolution, BoltV3MarketIdentityError> {
    if market_selection_timestamp_milliseconds < 0 {
        return Err(BoltV3MarketIdentityError::NegativeNowUnixSeconds {
            now_unix_seconds: market_selection_timestamp_milliseconds.div_euclid(1_000),
        });
    }
    let candidates =
        candidates_for_target(target, market_selection_timestamp_milliseconds / 1_000)?;
    let current = complete_updown_markets_for_slug(
        cache,
        target,
        venue,
        &candidates.current_market_slug,
        |market| {
            market.polymarket_market_start_timestamp_milliseconds
                <= market_selection_timestamp_milliseconds
                && market_selection_timestamp_milliseconds
                    < market.polymarket_market_end_timestamp_milliseconds
        },
    );
    if current.len() > 1 {
        return Ok(UpdownSelectedMarketResolution::Failed {
            failure_reason: UpdownSelectedMarketFailureReason::AmbiguousSelectedMarket,
        });
    }
    if let Some(selected_market) = current.into_iter().next() {
        return Ok(UpdownSelectedMarketResolution::Selected {
            role: UpdownSelectedMarketRole::Current,
            selected_market,
        });
    }

    let next = complete_updown_markets_for_slug(
        cache,
        target,
        venue,
        &candidates.next_market_slug,
        |market| {
            market.polymarket_market_start_timestamp_milliseconds
                > market_selection_timestamp_milliseconds
        },
    );
    if next.len() > 1 {
        return Ok(UpdownSelectedMarketResolution::Failed {
            failure_reason: UpdownSelectedMarketFailureReason::AmbiguousSelectedMarket,
        });
    }
    if let Some(selected_market) = next.into_iter().next() {
        return Ok(UpdownSelectedMarketResolution::Selected {
            role: UpdownSelectedMarketRole::Next,
            selected_market,
        });
    }

    let has_candidate_instruments =
        cache_contains_slug(cache, venue, &candidates.current_market_slug)
            || cache_contains_slug(cache, venue, &candidates.next_market_slug);
    Ok(UpdownSelectedMarketResolution::Failed {
        failure_reason: if has_candidate_instruments {
            UpdownSelectedMarketFailureReason::NoSelectedMarket
        } else {
            UpdownSelectedMarketFailureReason::InstrumentsNotInCache
        },
    })
}

pub fn resolve_updown_selected_markets_for_client_from_cache(
    cache: &Cache,
    plan: &MarketIdentityPlan,
    client_id_key: &str,
    venue: &Venue,
    market_selection_timestamp_milliseconds: i64,
) -> Result<Vec<UpdownTargetSelectedMarketResolution>, BoltV3MarketIdentityError> {
    plan.updown_targets
        .iter()
        .filter(|target| target.client_id_key == client_id_key)
        .map(|target| {
            Ok(UpdownTargetSelectedMarketResolution {
                strategy_instance_id: target.strategy_instance_id.clone(),
                configured_target_id: target.configured_target_id.clone(),
                resolution: resolve_updown_selected_market_from_cache(
                    cache,
                    target,
                    venue,
                    market_selection_timestamp_milliseconds,
                )?,
            })
        })
        .collect()
}

fn check_instrument_readiness(
    context: ProviderInstrumentReadinessContext<'_>,
) -> Result<Vec<ProviderInstrumentReadinessFact>, BoltV3MarketIdentityError> {
    let venue = Venue::new(context.venue_key);
    resolve_updown_selected_markets_for_client_from_cache(
        context.cache,
        context.plan,
        context.client_id_key,
        &venue,
        context.market_selection_timestamp_milliseconds,
    )
    .map(|resolutions| {
        resolutions
            .into_iter()
            .map(|target| {
                let (status, detail) = match target.resolution {
                    UpdownSelectedMarketResolution::Selected {
                        role,
                        selected_market,
                    } => (
                        ProviderInstrumentReadinessStatus::Ready,
                        format!(
                            "selected_market role={} market_slug={} up_instrument_id={} down_instrument_id={}",
                            selected_market_role_as_str(role),
                            selected_market.polymarket_market_slug,
                            selected_market.up_instrument_id,
                            selected_market.down_instrument_id
                        ),
                    ),
                    UpdownSelectedMarketResolution::Failed { failure_reason } => (
                        ProviderInstrumentReadinessStatus::Blocked,
                        updown_selected_market_failure_reason_as_str(failure_reason).to_string(),
                    ),
                };
                ProviderInstrumentReadinessFact {
                    client_id_key: context.client_id_key.to_string(),
                    strategy_instance_id: target.strategy_instance_id,
                    configured_target_id: target.configured_target_id,
                    status,
                    detail,
                }
            })
            .collect()
    })
}

fn complete_updown_markets_for_slug(
    cache: &Cache,
    target: &UpdownTargetPlan,
    venue: &Venue,
    market_slug: &str,
    role_predicate: impl Fn(&UpdownSelectedMarket) -> bool,
) -> Vec<UpdownSelectedMarket> {
    let mut pairs = BTreeMap::<UpdownMarketKey, UpdownInstrumentPair>::new();
    for instrument in cache.instruments(venue, None) {
        let Some((key, outcome, instrument_id)) = updown_instrument_row(instrument, market_slug)
        else {
            continue;
        };
        let pair = pairs.entry(key).or_default();
        if outcome.eq_ignore_ascii_case("up") {
            pair.up_instrument_id = Some(instrument_id);
        } else if outcome.eq_ignore_ascii_case("down") {
            pair.down_instrument_id = Some(instrument_id);
        }
    }

    pairs
        .into_iter()
        .filter_map(|(key, pair)| {
            Some(UpdownSelectedMarket {
                market_selection_type: target.market_selection_type.clone(),
                client_id: target.client_id_key.clone(),
                venue: venue.as_str().to_string(),
                rotating_market_family: updown::KEY.to_string(),
                polymarket_condition_id: key.condition_id,
                polymarket_market_slug: key.market_slug,
                polymarket_question_id: key.question_id,
                up_instrument_id: pair.up_instrument_id?,
                down_instrument_id: pair.down_instrument_id?,
                polymarket_market_start_timestamp_milliseconds: key.start_ms,
                polymarket_market_end_timestamp_milliseconds: key.end_ms,
            })
        })
        .filter(role_predicate)
        .collect()
}

fn cache_contains_slug(cache: &Cache, venue: &Venue, market_slug: &str) -> bool {
    cache
        .instruments(venue, None)
        .into_iter()
        .any(|instrument| instrument_market_slug(instrument) == Some(market_slug))
}

fn updown_instrument_row(
    instrument: &InstrumentAny,
    expected_market_slug: &str,
) -> Option<(UpdownMarketKey, String, String)> {
    let InstrumentAny::BinaryOption(binary) = instrument else {
        return None;
    };
    let market_slug = instrument_info_str(binary, "market_slug")?;
    if market_slug != expected_market_slug {
        return None;
    }
    let key = UpdownMarketKey {
        condition_id: instrument_info_str(binary, "condition_id")?.to_string(),
        market_slug: market_slug.to_string(),
        question_id: instrument_info_str(binary, "question_id")?.to_string(),
        start_ms: unix_nanos_to_millis(binary.activation_ns)?,
        end_ms: unix_nanos_to_millis(binary.expiration_ns)?,
    };
    let outcome = binary.outcome.map(|value| value.to_string())?;
    Some((key, outcome, binary.id.to_string()))
}

fn instrument_market_slug(instrument: &InstrumentAny) -> Option<&str> {
    let InstrumentAny::BinaryOption(binary) = instrument else {
        return None;
    };
    instrument_info_str(binary, "market_slug")
}

fn instrument_info_str<'a>(binary: &'a BinaryOption, key: &str) -> Option<&'a str> {
    binary.info.as_ref()?.get_str(key)
}

fn unix_nanos_to_millis(value: nautilus_core::UnixNanos) -> Option<i64> {
    i64::try_from(value.as_u64() / 1_000_000).ok()
}

fn selected_market_role_as_str(role: UpdownSelectedMarketRole) -> &'static str {
    match role {
        UpdownSelectedMarketRole::Current => "current",
        UpdownSelectedMarketRole::Next => "next",
    }
}

fn updown_selected_market_failure_reason_as_str(
    reason: UpdownSelectedMarketFailureReason,
) -> &'static str {
    match reason {
        UpdownSelectedMarketFailureReason::InstrumentsNotInCache => "instruments_not_in_cache",
        UpdownSelectedMarketFailureReason::NoSelectedMarket => "no_selected_market",
        UpdownSelectedMarketFailureReason::AmbiguousSelectedMarket => "ambiguous_selected_market",
    }
}

fn build_market_slug_filters_for_client(
    context: ProviderFamilyFilterContext<'_>,
) -> Vec<Arc<dyn InstrumentFilter>> {
    context
        .plan
        .updown_targets
        .iter()
        .filter(|target| target.client_id_key == context.client_id_key)
        .map(|target| build_market_slug_filter(target, context.clock.clone()))
        .collect()
}

fn build_market_slug_filter(
    target: &UpdownTargetPlan,
    clock: BoltV3MarketSelectionNowFn,
) -> Arc<dyn InstrumentFilter> {
    let asset = target.underlying_asset.clone();
    let token = target.cadence_slug_token.clone();
    let cadence = target.cadence_seconds;
    Arc::new(MarketSlugFilter::new(move || {
        let now = (clock)();
        match updown_period_pair(cadence, now) {
            Ok((current, next)) => vec![
                updown_market_slug(&asset, &token, current),
                updown_market_slug(&asset, &token, next),
            ],
            Err(error) => {
                log::warn!(
                    "bolt-v3 provider-family binding: skipping updown filter cycle (cadence={cadence}, now_unix_seconds={now}): {error}"
                );
                Vec::new()
            }
        }
    }))
}
