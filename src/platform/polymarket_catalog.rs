use anyhow::Context;
use chrono::{DateTime, Utc};
use futures_util::future::join_all;
use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_model::identifiers::InstrumentId;
use nautilus_network::http::{HttpClient, Method, USER_AGENT};
use nautilus_polymarket::common::urls::gamma_api_url;
use nautilus_polymarket::http::{
    gamma::PolymarketGammaRawHttpClient, models::GammaMarket, query::GetGammaEventsParams,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    str::FromStr,
};

use crate::{
    clients::polymarket::{
        PolymarketRulesetSelector, fetch_gamma_events_paginated,
        resolve_matching_events_for_selectors_with_gamma_client,
    },
    config::RulesetConfig,
    platform::{resolution_basis::parse_declared_resolution_basis, ruleset::CandidateMarket},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGammaEvent {
    #[serde(default)]
    markets: Vec<RawGammaMarket>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGammaMarket {
    id: String,
    x_axis_value: Option<String>,
    y_axis_value: Option<String>,
    lower_bound: Option<String>,
    upper_bound: Option<String>,
    group_item_threshold: Option<String>,
}

#[derive(Serialize)]
struct SlugParam<'a> {
    slug: &'a str,
}

pub async fn load_candidate_markets_for_ruleset(
    ruleset: &RulesetConfig,
    timeout_secs: u64,
) -> anyhow::Result<Vec<CandidateMarket>> {
    let raw_client = PolymarketGammaRawHttpClient::new(None, timeout_secs)
        .context("failed to build gamma raw client")?;
    load_candidate_markets_for_ruleset_with_gamma_client(ruleset, &raw_client, None).await
}

pub async fn load_candidate_markets_for_ruleset_with_gamma_client(
    ruleset: &RulesetConfig,
    client: &PolymarketGammaRawHttpClient,
    raw_base_url: Option<&str>,
) -> anyhow::Result<Vec<CandidateMarket>> {
    let selector: PolymarketRulesetSelector = ruleset
        .selector
        .clone()
        .try_into()
        .context("failed to parse polymarket selector")?;
    let events = load_events_for_selector(&selector, client)
        .await
        .context("failed to fetch gamma events")?;
    let price_to_beat_by_market_id = load_price_to_beat_map_for_events(
        &events,
        raw_base_url,
        ruleset.candidate_load_timeout_secs,
    )
    .await;
    let now = Utc::now();

    Ok(events
        .into_iter()
        .flat_map(|event| event.markets.into_iter())
        .filter_map(|market| {
            let price_to_beat = price_to_beat_by_market_id
                .get(&market.id)
                .copied()
                .flatten();
            translate_market(market, price_to_beat, now)
        })
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

async fn load_price_to_beat_map_for_events(
    events: &[nautilus_polymarket::http::models::GammaEvent],
    raw_base_url: Option<&str>,
    timeout_secs: u64,
) -> BTreeMap<String, Option<f64>> {
    let mut by_market_id = BTreeMap::new();
    let slugs: Vec<String> = events
        .iter()
        .filter_map(|event| event.slug.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if slugs.is_empty() {
        return by_market_id;
    }

    let base_url = raw_base_url
        .map(str::to_string)
        .unwrap_or_else(|| gamma_api_url().to_string());

    match fetch_raw_events_by_slugs(&slugs, &base_url, timeout_secs).await {
        Ok(raw_events) => {
            for event in raw_events {
                for market in event.markets {
                    by_market_id.insert(market.id.clone(), extract_price_to_beat(&market));
                }
            }
        }
        Err(error) => {
            log::warn!("failed to fetch raw polymarket anchors: {error:#}");
        }
    }

    by_market_id
}

async fn fetch_raw_events_by_slugs(
    slugs: &[String],
    base_url: &str,
    timeout_secs: u64,
) -> anyhow::Result<Vec<RawGammaEvent>> {
    let client = HttpClient::new(
        default_gamma_headers(),
        vec![],
        vec![],
        None,
        Some(timeout_secs),
        None,
    )
    .context("failed to build raw gamma http client")?;
    let requests = slugs.iter().cloned().map(|slug| {
        let client = client.clone();
        let url = format!("{}/events", base_url.trim_end_matches('/'));
        async move {
            let response = client
                .request_with_params(
                    Method::GET,
                    url,
                    Some(&SlugParam { slug: &slug }),
                    None,
                    None,
                    None,
                    None,
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;

            if !response.status.is_success() {
                return Err(anyhow::anyhow!(
                    "raw gamma events request for slug {slug} failed with status {}",
                    response.status.as_u16()
                ));
            }

            let page: Vec<RawGammaEvent> = serde_json::from_slice(&response.body)
                .context("failed to decode raw gamma events response")?;
            Ok::<_, anyhow::Error>((slug, page))
        }
    });

    let mut events = Vec::new();
    for result in join_all(requests).await {
        match result {
            Ok((_slug, mut page)) => events.append(&mut page),
            Err(error) => {
                log::warn!("failed to fetch raw polymarket anchor slug: {error:#}");
            }
        }
    }

    Ok(events)
}

fn default_gamma_headers() -> HashMap<String, String> {
    HashMap::from([
        (USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
    ])
}

fn extract_price_to_beat(market: &RawGammaMarket) -> Option<f64> {
    [
        market.x_axis_value.as_deref(),
        market.y_axis_value.as_deref(),
        market.lower_bound.as_deref(),
        market.upper_bound.as_deref(),
        market.group_item_threshold.as_deref(),
    ]
    .into_iter()
    .find_map(parse_anchor_price)
}

fn parse_anchor_price(value: Option<&str>) -> Option<f64> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    let parsed = value.parse::<f64>().ok()?;
    (parsed.is_finite() && parsed > 0.0).then_some(parsed)
}

fn translate_market(
    market: GammaMarket,
    price_to_beat: Option<f64>,
    now: DateTime<Utc>,
) -> Option<CandidateMarket> {
    match translate_market_result(market, price_to_beat, now) {
        Ok(candidate_market) => Some(candidate_market),
        Err((market_id, reason)) => {
            log::warn!("skipping candidate market {market_id}: {reason}");
            None
        }
    }
}

fn translate_market_result(
    market: GammaMarket,
    price_to_beat: Option<f64>,
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
        price_to_beat,
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
        let candidate =
            translate_market_result(parse_market(valid_market_json()), None, Utc::now())
                .expect("valid market should translate");
        assert_eq!(candidate.market_id, "market-good");
        assert_eq!(candidate.instrument_id, "0xcondition1-111.POLYMARKET");
    }

    #[test]
    fn translate_market_reports_invalid_outcome_labels() {
        let mut market = valid_market_json();
        market["outcomes"] = json!("[\"Yes\",\"No\"]");
        let reason = translate_market_result(parse_market(market), None, Utc::now())
            .expect_err("invalid outcomes should produce a drop reason")
            .1;
        assert!(reason.contains("unsupported outcome labels"), "{reason}");
    }

    #[test]
    fn translate_market_reports_malformed_token_ids() {
        let mut market = valid_market_json();
        market["clobTokenIds"] = json!("not-json");
        let reason = translate_market_result(parse_market(market), None, Utc::now())
            .expect_err("malformed token ids should produce a drop reason")
            .1;
        assert!(reason.contains("unsupported outcome labels"), "{reason}");
    }

    #[test]
    fn translate_market_reports_missing_start_date() {
        let mut market = valid_market_json();
        market.as_object_mut().unwrap().remove("startDate");
        let reason = translate_market_result(parse_market(market), None, Utc::now())
            .expect_err("missing startDate should produce a drop reason")
            .1;
        assert!(reason.contains("missing startDate"), "{reason}");
    }

    #[test]
    fn translate_market_reports_invalid_start_date() {
        let mut market = valid_market_json();
        market["startDate"] = json!("not-a-date");
        let reason = translate_market_result(parse_market(market), None, Utc::now())
            .expect_err("invalid startDate should produce a drop reason")
            .1;
        assert!(reason.contains("invalid startDate"), "{reason}");
    }

    #[test]
    fn translate_market_reports_missing_accepting_orders() {
        let mut market = valid_market_json();
        market.as_object_mut().unwrap().remove("acceptingOrders");
        let reason = translate_market_result(parse_market(market), None, Utc::now())
            .expect_err("missing acceptingOrders should produce a drop reason")
            .1;
        assert!(reason.contains("missing acceptingOrders"), "{reason}");
    }

    #[test]
    fn translate_market_reports_missing_liquidity_num() {
        let mut market = valid_market_json();
        market.as_object_mut().unwrap().remove("liquidityNum");
        let reason = translate_market_result(parse_market(market), None, Utc::now())
            .expect_err("missing liquidityNum should produce a drop reason")
            .1;
        assert!(reason.contains("missing liquidityNum"), "{reason}");
    }

    #[test]
    fn translate_market_reports_missing_end_date() {
        let mut market = valid_market_json();
        market.as_object_mut().unwrap().remove("endDate");
        let reason = translate_market_result(parse_market(market), None, Utc::now())
            .expect_err("missing endDate should produce a drop reason")
            .1;
        assert!(reason.contains("missing endDate"), "{reason}");
    }

    #[test]
    fn translate_market_reports_invalid_end_date() {
        let mut market = valid_market_json();
        market["endDate"] = json!("not-a-date");
        let reason = translate_market_result(parse_market(market), None, Utc::now())
            .expect_err("invalid endDate should produce a drop reason")
            .1;
        assert!(reason.contains("invalid endDate"), "{reason}");
    }

    #[test]
    fn extract_price_to_beat_prefers_axis_values_before_thresholds() {
        let market = RawGammaMarket {
            id: "market-good".to_string(),
            x_axis_value: Some("3100.25".to_string()),
            y_axis_value: Some("3200.50".to_string()),
            lower_bound: Some("100.0".to_string()),
            upper_bound: Some("200.0".to_string()),
            group_item_threshold: Some("300.0".to_string()),
        };

        assert_eq!(extract_price_to_beat(&market), Some(3100.25));
    }
}
