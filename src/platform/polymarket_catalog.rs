use anyhow::Context;
use chrono::{DateTime, Utc};
use nautilus_model::identifiers::InstrumentId;
use nautilus_polymarket::http::{
    gamma::PolymarketGammaRawHttpClient, models::GammaMarket, query::GetGammaEventsParams,
};
use std::str::FromStr;

use crate::{
    clients::polymarket::{
        PolymarketRulesetSelector, fetch_gamma_events_paginated,
        resolve_matching_events_for_selectors_with_gamma_client,
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

    resolve_matching_events_for_selectors_with_gamma_client(std::slice::from_ref(selector), client)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn translate_market(market: GammaMarket, now: DateTime<Utc>) -> Option<CandidateMarket> {
    match translate_market_result(market, now) {
        Ok(candidate_market) => Some(candidate_market),
        Err((market_id, reason)) => {
            log::warn!("skipping candidate market {market_id}: {reason}");
            None
        }
    }
}

fn translate_market_result(
    market: GammaMarket,
    now: DateTime<Utc>,
) -> Result<CandidateMarket, (String, String)> {
    let market_id = market.id.clone();
    let declared_resolution_basis = parse_declared_resolution_basis(market.description.as_deref())
        .ok_or_else(|| {
            (
                market_id.clone(),
                "could not parse declared resolution basis from description".to_string(),
            )
        })?;
    let (up_token_id, down_token_id) =
        binary_up_down_token_ids(&market.clob_token_ids, &market.outcomes).ok_or_else(|| {
            (
                market_id.clone(),
                "unsupported outcome labels or malformed token ids".to_string(),
            )
        })?;
    let start_date = market
        .start_date
        .as_deref()
        .ok_or_else(|| (market_id.clone(), "missing startDate".to_string()))?;
    let start_ts_ms = parse_timestamp_ms(start_date).ok_or_else(|| {
        (
            market_id.clone(),
            format!("invalid startDate {:?}", start_date),
        )
    })?;
    let instrument_id = polymarket_instrument_id(&market.condition_id, &up_token_id).to_string();
    let accepting_orders = market
        .accepting_orders
        .ok_or_else(|| (market_id.clone(), "missing acceptingOrders".to_string()))?;
    let liquidity_num = market
        .liquidity_num
        .ok_or_else(|| (market_id.clone(), "missing liquidityNum".to_string()))?;
    let end_date = market
        .end_date
        .as_deref()
        .ok_or_else(|| (market_id.clone(), "missing endDate".to_string()))?;
    let seconds_to_end = seconds_to_end(now, end_date)
        .ok_or_else(|| (market_id.clone(), format!("invalid endDate {:?}", end_date)))?;

    Ok(CandidateMarket {
        market_id: market.id,
        instrument_id,
        condition_id: market.condition_id,
        up_token_id,
        down_token_id,
        start_ts_ms,
        declared_resolution_basis,
        accepting_orders,
        liquidity_num,
        seconds_to_end,
    })
}

#[must_use]
pub fn polymarket_instrument_id(condition_id: &str, token_id: &str) -> InstrumentId {
    InstrumentId::from_str(&format!("{condition_id}-{token_id}.POLYMARKET"))
        .expect("polymarket instrument id format should be valid")
}

fn binary_up_down_token_ids(clob_token_ids: &str, outcomes: &str) -> Option<(String, String)> {
    let token_ids = serde_json::from_str::<Vec<String>>(clob_token_ids).ok()?;
    let outcomes = serde_json::from_str::<Vec<String>>(outcomes).ok()?;
    if token_ids.len() != 2 || outcomes.len() != 2 {
        return None;
    }

    let mut up_token_id = None;
    let mut down_token_id = None;
    for (outcome, token_id) in outcomes.into_iter().zip(token_ids.into_iter()) {
        match outcome.as_str() {
            "Up" => up_token_id = Some(token_id),
            "Down" => down_token_id = Some(token_id),
            _ => return None,
        }
    }

    Some((up_token_id?, down_token_id?))
}

fn parse_timestamp_ms(value: &str) -> Option<u64> {
    let timestamp_ms = DateTime::parse_from_rfc3339(value).ok()?.timestamp_millis();
    Some(timestamp_ms.max(0) as u64)
}

fn seconds_to_end(now: DateTime<Utc>, end_date: &str) -> Option<u64> {
    let end_time = DateTime::parse_from_rfc3339(end_date)
        .ok()?
        .with_timezone(&Utc);
    let delta = end_time.signed_duration_since(now).num_seconds();
    Some(delta.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_market_json() -> serde_json::Value {
        json!({
            "id": "market-good",
            "questionID": "0xquestion1",
            "conditionId": "0xcondition1",
            "clobTokenIds": "[\"111\",\"222\"]",
            "outcomes": "[\"Up\",\"Down\"]",
            "question": "Will BTC finish green?",
            "description": "This market will resolve to \"Yes\" if the Binance 1 minute candle for BTCUSDT has a final close above the opening price. The resolution source for this market is Binance, specifically the BTCUSDT \"Close\" prices available with \"1m\" and \"Candles\" selected on the top bar.",
            "startDate": (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339(),
            "acceptingOrders": true,
            "liquidityNum": 4567.0,
            "endDate": (Utc::now() + chrono::Duration::minutes(20)).to_rfc3339(),
            "slug": "market-good"
        })
    }

    fn parse_market(value: serde_json::Value) -> GammaMarket {
        serde_json::from_value(value).expect("gamma market fixture should parse")
    }

    #[test]
    fn translate_market_result_accepts_valid_market() {
        let candidate = translate_market_result(parse_market(valid_market_json()), Utc::now())
            .expect("valid market should translate");
        assert_eq!(candidate.market_id, "market-good");
        assert_eq!(candidate.instrument_id, "0xcondition1-111.POLYMARKET");
    }

    #[test]
    fn translate_market_reports_invalid_outcome_labels() {
        let mut market = valid_market_json();
        market["outcomes"] = json!("[\"Yes\",\"No\"]");
        let reason = translate_market_result(parse_market(market), Utc::now())
            .expect_err("invalid outcomes should produce a drop reason")
            .1;
        assert!(reason.contains("unsupported outcome labels"), "{reason}");
    }

    #[test]
    fn translate_market_reports_malformed_token_ids() {
        let mut market = valid_market_json();
        market["clobTokenIds"] = json!("not-json");
        let reason = translate_market_result(parse_market(market), Utc::now())
            .expect_err("malformed token ids should produce a drop reason")
            .1;
        assert!(reason.contains("unsupported outcome labels"), "{reason}");
    }

    #[test]
    fn translate_market_reports_missing_start_date() {
        let mut market = valid_market_json();
        market.as_object_mut().unwrap().remove("startDate");
        let reason = translate_market_result(parse_market(market), Utc::now())
            .expect_err("missing startDate should produce a drop reason")
            .1;
        assert!(reason.contains("missing startDate"), "{reason}");
    }

    #[test]
    fn translate_market_reports_invalid_start_date() {
        let mut market = valid_market_json();
        market["startDate"] = json!("not-a-date");
        let reason = translate_market_result(parse_market(market), Utc::now())
            .expect_err("invalid startDate should produce a drop reason")
            .1;
        assert!(reason.contains("invalid startDate"), "{reason}");
    }

    #[test]
    fn translate_market_reports_missing_accepting_orders() {
        let mut market = valid_market_json();
        market.as_object_mut().unwrap().remove("acceptingOrders");
        let reason = translate_market_result(parse_market(market), Utc::now())
            .expect_err("missing acceptingOrders should produce a drop reason")
            .1;
        assert!(reason.contains("missing acceptingOrders"), "{reason}");
    }

    #[test]
    fn translate_market_reports_missing_liquidity_num() {
        let mut market = valid_market_json();
        market.as_object_mut().unwrap().remove("liquidityNum");
        let reason = translate_market_result(parse_market(market), Utc::now())
            .expect_err("missing liquidityNum should produce a drop reason")
            .1;
        assert!(reason.contains("missing liquidityNum"), "{reason}");
    }

    #[test]
    fn translate_market_reports_missing_end_date() {
        let mut market = valid_market_json();
        market.as_object_mut().unwrap().remove("endDate");
        let reason = translate_market_result(parse_market(market), Utc::now())
            .expect_err("missing endDate should produce a drop reason")
            .1;
        assert!(reason.contains("missing endDate"), "{reason}");
    }

    #[test]
    fn translate_market_reports_invalid_end_date() {
        let mut market = valid_market_json();
        market["endDate"] = json!("not-a-date");
        let reason = translate_market_result(parse_market(market), Utc::now())
            .expect_err("invalid endDate should produce a drop reason")
            .1;
        assert!(reason.contains("invalid endDate"), "{reason}");
    }
}
