use anyhow::Context;
use chrono::{DateTime, Utc};
use futures_util::future::join_all;
use nautilus_model::identifiers::InstrumentId;
use nautilus_polymarket::http::{
    gamma::PolymarketGammaRawHttpClient, models::GammaMarket, query::GetGammaEventsParams,
};
use std::{collections::BTreeSet, str::FromStr};

use crate::{
    clients::polymarket::{
        PolymarketRulesetSelector, PolymarketSelectorState, fetch_gamma_events_paginated,
        polymarket_prefix_discovery_for_ruleset,
    },
    config::RulesetConfig,
    platform::{resolution_basis::parse_declared_resolution_basis, ruleset::CandidateMarket},
};

pub async fn load_candidate_markets_for_ruleset(
    ruleset: &RulesetConfig,
    timeout_secs: u64,
    selector_state: Option<PolymarketSelectorState>,
) -> anyhow::Result<Vec<CandidateMarket>> {
    let raw_client = PolymarketGammaRawHttpClient::new(None, timeout_secs)
        .context("failed to build gamma raw client")?;
    load_candidate_markets_for_ruleset_with_gamma_client(ruleset, &raw_client, selector_state).await
}

pub async fn load_candidate_markets_for_ruleset_with_gamma_client(
    ruleset: &RulesetConfig,
    client: &PolymarketGammaRawHttpClient,
    selector_state: Option<PolymarketSelectorState>,
) -> anyhow::Result<Vec<CandidateMarket>> {
    let selector: PolymarketRulesetSelector = ruleset
        .selector
        .clone()
        .try_into()
        .context("failed to parse polymarket selector")?;
    let events = load_events_for_selector(ruleset, &selector, selector_state.as_ref(), client)
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
    ruleset: &RulesetConfig,
    selector: &PolymarketRulesetSelector,
    selector_state: Option<&PolymarketSelectorState>,
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

    // Prefix selectors require a selector_state populated by the production wiring
    // (build_selector_state at startup + selector refresh task). Missing state here
    // would silently trigger the old fallback that performed a broad Gamma discovery
    // per poll tick, contradicting the "single shared PolymarketRulesetSetup" ethos
    // and exercising a code path that production never reaches.
    let Some(selector_state) = selector_state else {
        anyhow::bail!(
            "polymarket ruleset validation: prefix selector ruleset {} requires selector_state, got None",
            ruleset.id
        );
    };

    let prefix_discovery = polymarket_prefix_discovery_for_ruleset(ruleset)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?
        .ok_or_else(|| anyhow::anyhow!("missing prefix discovery for prefix selector"))?;
    let event_slugs = selector_state.event_slugs_for_discovery(&prefix_discovery);
    if !event_slugs.is_empty() {
        return load_events_by_event_slugs(&event_slugs, client).await;
    }

    anyhow::bail!(
        "selector state empty for tag_slug={} prefix={:?}; failing closed until selector refresh repopulates event slugs",
        selector.tag_slug,
        selector.event_slug_prefix.as_deref()
    );
}

async fn load_events_by_event_slugs(
    event_slugs: &[String],
    client: &PolymarketGammaRawHttpClient,
) -> anyhow::Result<Vec<nautilus_polymarket::http::models::GammaEvent>> {
    let unique_event_slugs: Vec<String> = event_slugs
        .iter()
        .map(|event_slug| event_slug.trim())
        .filter(|event_slug| !event_slug.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let requests = unique_event_slugs.iter().map(|event_slug| async move {
        client
            .get_gamma_events_by_slug(event_slug)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    });

    let mut events = Vec::new();
    for result in join_all(requests).await {
        let mut page = result?;
        events.append(&mut page);
    }

    Ok(events)
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
    use crate::clients::polymarket::PolymarketSelectorState;
    use crate::config::RulesetVenueKind;
    use chrono::Duration as ChronoDuration;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

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

    fn ruleset_with_prefix(prefix: &str) -> RulesetConfig {
        RulesetConfig {
            id: "btc-5m".to_string(),
            venue: RulesetVenueKind::Polymarket,
            selector: toml::toml! {
                tag_slug = "bitcoin"
                event_slug_prefix = prefix
            }
            .into(),
            resolution_basis: "binance_btcusdt_1m".to_string(),
            min_time_to_expiry_secs: 120,
            max_time_to_expiry_secs: 1_800,
            min_liquidity_num: 1_000.0,
            require_accepting_orders: true,
            freeze_before_end_secs: 300,
            selector_poll_interval_ms: 1_000,
            candidate_load_timeout_secs: 30,
        }
    }

    fn parse_request_target(request: &str) -> (&str, HashMap<String, String>) {
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .expect("request line should include path");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let params = query
            .split('&')
            .filter(|part| !part.is_empty())
            .filter_map(|part| {
                let (key, value) = part.split_once('=')?;
                Some((key.to_string(), value.to_string()))
            })
            .collect();
        (path, params)
    }

    async fn spawn_test_server(
        response_bodies: Vec<serde_json::Value>,
    ) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
        let request_count = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bodies = Arc::new(response_bodies);
        let request_counter = Arc::clone(&request_count);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let bodies = Arc::clone(&bodies);
                let request_counter = Arc::clone(&request_counter);
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let (path, params) = parse_request_target(&request);
                    request_counter.fetch_add(1, Ordering::Relaxed);

                    let body = if path == "/events" {
                        if let Some(slug) = params.get("slug") {
                            let matching_events: Vec<serde_json::Value> = bodies
                                .iter()
                                .flat_map(|value| value.as_array().cloned().unwrap_or_default())
                                .filter(|event| {
                                    event.get("slug").and_then(serde_json::Value::as_str)
                                        == Some(slug.as_str())
                                })
                                .collect();
                            serde_json::Value::Array(matching_events).to_string()
                        } else if params.get("tag_slug").map(String::as_str) == Some("bitcoin") {
                            bodies
                                .first()
                                .cloned()
                                .unwrap_or_else(|| json!([]))
                                .to_string()
                        } else {
                            json!([]).to_string()
                        }
                    } else {
                        json!([]).to_string()
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });

        (addr, request_count)
    }

    fn event_with_slug_and_markets(
        id: &str,
        slug: &str,
        title: &str,
        markets: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        json!({
            "id": id,
            "slug": slug,
            "title": title,
            "markets": markets
        })
    }

    fn valid_market_with(id: &str, clob_token_ids: &str, end_date: String) -> serde_json::Value {
        json!({
            "id": id,
            "questionID": "0xquestion1",
            "conditionId": "0xcondition1",
            "clobTokenIds": clob_token_ids,
            "outcomes": "[\"Up\",\"Down\"]",
            "question": "Will BTC finish green?",
            "description": "This market will resolve to \"Yes\" if the Binance 1 minute candle for BTCUSDT has a final close above the opening price. The resolution source for this market is Binance, specifically the BTCUSDT \"Close\" prices available with \"1m\" and \"Candles\" selected on the top bar.",
            "startDate": (Utc::now() - ChronoDuration::minutes(5)).to_rfc3339(),
            "acceptingOrders": true,
            "liquidityNum": 4567.0,
            "endDate": end_date,
            "slug": id
        })
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
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prefix_selector_catalog_uses_selector_state_event_slugs_in_hot_path() {
        let end_date = (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339();
        let (addr, request_count) = spawn_test_server(vec![
            json!([event_with_slug_and_markets(
                "event-tag-only",
                "bitcoin-15m-beta",
                "Bitcoin 15m",
                vec![valid_market_with(
                    "market-prefix-miss",
                    "[\"333\",\"444\"]",
                    end_date.clone(),
                )],
            )]),
            json!([event_with_slug_and_markets(
                "event-slug-hit",
                "bitcoin-5m-alpha",
                "Bitcoin 5m",
                vec![valid_market_with(
                    "market-prefix-hit",
                    "[\"111\",\"222\"]",
                    end_date,
                )],
            )]),
        ])
        .await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let ruleset = ruleset_with_prefix("bitcoin-5m");
        let selector_state = PolymarketSelectorState::new(vec![(
            polymarket_prefix_discovery_for_ruleset(&ruleset)
                .unwrap()
                .unwrap(),
            vec!["bitcoin-5m-alpha".to_string()],
        )]);

        let markets = load_candidate_markets_for_ruleset_with_gamma_client(
            &ruleset,
            &client,
            Some(selector_state),
        )
        .await
        .unwrap();

        assert_eq!(request_count.load(Ordering::Relaxed), 2);
        assert_eq!(markets.len(), 1);
        assert_eq!(markets[0].market_id, "market-prefix-hit");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prefix_selector_catalog_does_not_rediscover_when_selector_state_is_empty() {
        let (addr, request_count) = spawn_test_server(vec![json!([event_with_slug_and_markets(
            "event-slug-hit",
            "bitcoin-5m-alpha",
            "Bitcoin 5m",
            vec![valid_market_with(
                "market-prefix-hit",
                "[\"111\",\"222\"]",
                (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339(),
            )],
        )])])
        .await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let ruleset = ruleset_with_prefix("bitcoin-5m");
        let selector_state = PolymarketSelectorState::new(vec![(
            polymarket_prefix_discovery_for_ruleset(&ruleset)
                .unwrap()
                .unwrap(),
            vec![],
        )]);

        let err = load_candidate_markets_for_ruleset_with_gamma_client(
            &ruleset,
            &client,
            Some(selector_state),
        )
        .await
        .expect_err("empty selector state should fail closed");

        assert!(
            format!("{err:#}").contains("selector state empty"),
            "unexpected error: {err:#}"
        );
        assert_eq!(request_count.load(Ordering::Relaxed), 0);
    }
}
