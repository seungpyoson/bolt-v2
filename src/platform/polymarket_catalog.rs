use anyhow::Context;
use chrono::{DateTime, Utc};
use nautilus_polymarket::http::{
    gamma::PolymarketGammaRawHttpClient, models::GammaMarket, query::GetGammaEventsParams,
};

use crate::{
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
    let events = fetch_gamma_events_paginated(
        client,
        GetGammaEventsParams {
            tag_slug: Some(ruleset.tag_slug.clone()),
            ..Default::default()
        },
    )
    .await
    .context("failed to fetch gamma events")?;
    let now = Utc::now();

    Ok(events
        .into_iter()
        .flat_map(|event| event.markets.into_iter())
        .filter_map(|market| translate_market(market, &ruleset.tag_slug, now))
        .collect())
}

async fn fetch_gamma_events_paginated(
    client: &PolymarketGammaRawHttpClient,
    base_params: GetGammaEventsParams,
) -> anyhow::Result<Vec<nautilus_polymarket::http::models::GammaEvent>> {
    const PAGE_LIMIT: u32 = 100;

    let page_size = base_params.limit.unwrap_or(PAGE_LIMIT);
    let max_events = base_params.max_events;
    let mut all_events = Vec::new();
    let mut offset = base_params.offset.unwrap_or(0);

    loop {
        let page = client
            .get_gamma_events(GetGammaEventsParams {
                limit: Some(page_size),
                offset: Some(offset),
                ..base_params.clone()
            })
            .await?;
        let page_len = page.len() as u32;
        all_events.extend(page);

        if let Some(cap) = max_events
            && all_events.len() as u32 >= cap
        {
            all_events.truncate(cap as usize);
            break;
        }

        if page_len < page_size {
            break;
        }

        offset += page_size;
    }

    Ok(all_events)
}

fn translate_market(
    market: GammaMarket,
    tag_slug: &str,
    now: DateTime<Utc>,
) -> Option<CandidateMarket> {
    let declared_resolution_basis =
        parse_declared_resolution_basis(market.description.as_deref())?.to_string();
    let instrument_id = first_token_id(&market.clob_token_ids)?;
    let accepting_orders = market.accepting_orders?;
    let liquidity_num = market.liquidity_num?;
    let end_date = market.end_date?;
    let seconds_to_end = seconds_to_end(now, &end_date)?;

    Some(CandidateMarket {
        market_id: market.id,
        instrument_id,
        // Gamma event queries are scoped to a single ruleset slug in phase 1.
        tag_slug: tag_slug.to_string(),
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
