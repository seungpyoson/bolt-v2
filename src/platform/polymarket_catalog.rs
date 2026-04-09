use anyhow::Context;
use chrono::{DateTime, Utc};
use nautilus_network::http::{HttpClient, Method, USER_AGENT};
use nautilus_polymarket::http::query::GetGammaEventsParams;
use serde::Deserialize;

use crate::{
    config::RulesetConfig,
    platform::{resolution_basis::parse_declared_resolution_basis, ruleset::CandidateMarket},
};

pub fn load_candidate_markets_for_ruleset(
    ruleset: &RulesetConfig,
    timeout_secs: u64,
) -> anyhow::Result<Vec<CandidateMarket>> {
    let ruleset_tag_slug = ruleset.tag_slug.clone();
    let base_url = std::env::var("POLYMARKET_GAMMA_URL").ok();

    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to build catalog runtime")?
            .block_on(async move {
                let client = HttpClient::new(
                    gamma_default_headers(),
                    vec![],
                    vec![],
                    None,
                    Some(timeout_secs),
                    None,
                )
                .context("failed to build gamma http client")?;
                let response = client
                    .request_with_params(
                        Method::GET,
                        gamma_events_url(base_url.as_deref()),
                        Some(&GetGammaEventsParams {
                            slug: Some(ruleset_tag_slug.clone()),
                            ..Default::default()
                        }),
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                    .context("failed to fetch gamma events")?;
                anyhow::ensure!(
                    response.status.is_success(),
                    "gamma events request failed with status {}",
                    response.status.as_u16()
                );

                let events: Vec<GammaCatalogEvent> = serde_json::from_slice(&response.body)
                    .context("failed to decode gamma events")?;
                let now = Utc::now();

                Ok(events
                    .into_iter()
                    .flat_map(|event| event.markets.into_iter())
                    .filter_map(|market| market.try_into_candidate(&ruleset_tag_slug, now))
                    .collect())
            })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("catalog loader thread panicked"))?
}

fn gamma_default_headers() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([
        (
            USER_AGENT.to_string(),
            nautilus_core::consts::NAUTILUS_USER_AGENT.to_string(),
        ),
        ("Content-Type".to_string(), "application/json".to_string()),
    ])
}

fn gamma_events_url(base_url: Option<&str>) -> String {
    let base = base_url.unwrap_or(nautilus_polymarket::common::urls::gamma_api_url());
    format!("{}/events", base.trim_end_matches('/'))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaCatalogEvent {
    #[serde(default)]
    markets: Vec<GammaCatalogMarket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaCatalogMarket {
    id: String,
    clob_token_ids: String,
    description: Option<String>,
    resolution_source: Option<String>,
    accepting_orders: Option<bool>,
    liquidity_num: Option<f64>,
    end_date: Option<String>,
}

impl GammaCatalogMarket {
    fn try_into_candidate(
        self,
        ruleset_tag_slug: &str,
        now: DateTime<Utc>,
    ) -> Option<CandidateMarket> {
        let declared_resolution_basis = parse_declared_resolution_basis(
            self.resolution_source.as_deref(),
            self.description.as_deref(),
        )?;
        let instrument_id = first_token_id(&self.clob_token_ids)?;
        let accepting_orders = self.accepting_orders?;
        let liquidity_num = self.liquidity_num?;
        let end_date = self.end_date?;
        let seconds_to_end = seconds_to_end(now, &end_date)?;

        Some(CandidateMarket {
            market_id: self.id,
            instrument_id,
            // Gamma event queries are already scoped to one ruleset slug in phase 1.
            tag_slug: ruleset_tag_slug.to_string(),
            declared_resolution_basis,
            accepting_orders,
            liquidity_num,
            seconds_to_end,
        })
    }
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
