use anyhow::Context;
use chrono::{DateTime, Utc};
use nautilus_polymarket::http::{
    gamma::PolymarketGammaRawHttpClient, models::GammaMarket, query::GetGammaEventsParams,
};

use crate::{
    clients::polymarket::{
        PolymarketRulesetSelector, fetch_gamma_events_paginated,
        resolve_event_slugs_for_selectors_with_gamma_client,
    },
    config::RulesetConfig,
    platform::{resolution_basis::parse_declared_resolution_basis, ruleset::CandidateMarket},
};

pub async fn load_candidate_markets_for_ruleset(
    ruleset: &RulesetConfig,
    timeout_secs: u64,
) -> anyhow::Result<Vec<CandidateMarket>> {
    let raw_client = PolymarketGammaRawHttpClient::new(None, timeout_secs)
        .context("failed to build gamma raw client")?;
    load_candidate_markets_for_ruleset_with_gamma_client(ruleset, &raw_client).await
}

pub async fn load_candidate_markets_for_ruleset_with_gamma_client(
    ruleset: &RulesetConfig,
    client: &PolymarketGammaRawHttpClient,
) -> anyhow::Result<Vec<CandidateMarket>> {
    let selector: PolymarketRulesetSelector = ruleset
        .selector
        .clone()
        .try_into()
        .context("failed to parse polymarket selector")?;
    let events = load_events_for_selector(&selector, client)
        .await
        .context("failed to fetch gamma events")?;
    let now = Utc::now();

    Ok(events
        .into_iter()
        .flat_map(|event| event.markets.into_iter())
        .filter_map(|market| translate_market(market, now))
        .collect())
}

async fn load_events_for_selector(
    selector: &PolymarketRulesetSelector,
    client: &PolymarketGammaRawHttpClient,
) -> anyhow::Result<Vec<nautilus_polymarket::http::models::GammaEvent>> {
    if selector.event_slug_prefix.is_none() {
        return fetch_gamma_events_paginated(
            client,
            GetGammaEventsParams {
                tag_slug: Some(selector.tag_slug.clone()),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()));
    }

    let event_slugs =
        resolve_event_slugs_for_selectors_with_gamma_client(std::slice::from_ref(selector), client)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mut all_events = Vec::new();
    for event_slug in event_slugs {
        let mut events = fetch_gamma_events_paginated(
            client,
            GetGammaEventsParams {
                slug: Some(event_slug),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        all_events.append(&mut events);
    }
    Ok(all_events)
}

fn translate_market(market: GammaMarket, now: DateTime<Utc>) -> Option<CandidateMarket> {
    let declared_resolution_basis = match parse_declared_resolution_basis(
        market.description.as_deref(),
    ) {
        Some(basis) => basis,
        None => {
            log::warn!(
                "skipping candidate market {}: could not parse declared resolution basis from description",
                market.id
            );
            return None;
        }
    };
    let instrument_id = first_token_id(&market.clob_token_ids)?;
    let accepting_orders = market.accepting_orders?;
    let liquidity_num = market.liquidity_num?;
    let end_date = market.end_date?;
    let seconds_to_end = seconds_to_end(now, &end_date)?;

    Some(CandidateMarket {
        market_id: market.id,
        instrument_id,
        declared_resolution_basis,
        accepting_orders,
        liquidity_num,
        seconds_to_end,
    })
}

fn first_token_id(clob_token_ids: &str) -> Option<String> {
    serde_json::from_str::<Vec<String>>(clob_token_ids)
        .ok()?
        .into_iter()
        .next()
}

fn seconds_to_end(now: DateTime<Utc>, end_date: &str) -> Option<u64> {
    let end_time = DateTime::parse_from_rfc3339(end_date)
        .ok()?
        .with_timezone(&Utc);
    let delta = end_time.signed_duration_since(now).num_seconds();
    Some(delta.max(0) as u64)
}
