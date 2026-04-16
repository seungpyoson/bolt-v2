use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use nautilus_model::identifiers::{AccountId, TraderId};
use nautilus_polymarket::http::query::GetGammaEventsParams;
use nautilus_polymarket::{
    common::credential::Secrets as PolymarketSecrets,
    common::enums::SignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    factories::{PolymarketDataClientFactory, PolymarketExecutionClientFactory},
    filters::{EventParamsFilter, EventSlugFilter, InstrumentFilter, NewMarketPredicateFilter},
    http::{
        clob::PolymarketClobHttpClient, gamma::PolymarketGammaRawHttpClient, models::GammaEvent,
    },
};
use serde::Deserialize;
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use toml::Value;

use crate::config::{RulesetConfig, RulesetVenueKind};
use crate::secrets::ResolvedPolymarketSecrets;

pub mod fees;

pub use fees::{FeeProvider, PolymarketClobFeeProvider};

#[derive(Debug, Deserialize)]
pub struct PolymarketDataClientInput {
    #[serde(default)]
    pub subscribe_new_markets: bool,
    #[serde(default = "default_update_instruments_interval_mins")]
    pub update_instruments_interval_mins: u64,
    #[serde(default = "default_gamma_refresh_interval_secs")]
    pub gamma_refresh_interval_secs: u64,
    #[serde(default = "default_ws_max_subscriptions")]
    pub ws_max_subscriptions: usize,
    #[serde(default)]
    pub event_slugs: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PolymarketDataClientCommonInput {
    #[serde(default)]
    subscribe_new_markets: bool,
    #[serde(default = "default_update_instruments_interval_mins")]
    update_instruments_interval_mins: u64,
    #[serde(default = "default_gamma_refresh_interval_secs")]
    gamma_refresh_interval_secs: u64,
    #[serde(default = "default_ws_max_subscriptions")]
    ws_max_subscriptions: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolymarketRulesetSelector {
    pub tag_slug: String,
    #[serde(default)]
    pub event_slug_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolymarketPrefixDiscovery {
    pub selector: PolymarketRulesetSelector,
    pub min_time_to_expiry_secs: u64,
    pub max_time_to_expiry_secs: u64,
}

#[derive(Clone, Debug)]
pub struct PolymarketSelectorState {
    prefix_selectors: Vec<PolymarketRulesetSelector>,
    event_slugs: Arc<RwLock<Vec<String>>>,
}

impl PolymarketSelectorState {
    pub(crate) fn new(
        prefix_selectors: Vec<PolymarketRulesetSelector>,
        event_slugs: Vec<String>,
    ) -> Self {
        Self {
            prefix_selectors,
            event_slugs: Arc::new(RwLock::new(event_slugs)),
        }
    }

    fn current_event_slugs(&self) -> Vec<String> {
        self.event_slugs
            .read()
            .expect("selector event slugs lock poisoned")
            .clone()
    }

    fn replace_event_slugs(&self, event_slugs: Vec<String>) {
        *self
            .event_slugs
            .write()
            .expect("selector event slugs lock poisoned") = event_slugs;
    }

    pub(crate) fn event_slugs_for_selector(
        &self,
        selector: &PolymarketRulesetSelector,
    ) -> Vec<String> {
        let event_slugs = self.current_event_slugs();
        match selector.event_slug_prefix.as_deref() {
            Some(prefix) => event_slugs
                .into_iter()
                .filter(|event_slug| event_slug.starts_with(prefix))
                .collect(),
            None => event_slugs,
        }
    }

    fn accepts_new_market_slug(&self, slug: &str) -> bool {
        if self
            .current_event_slugs()
            .iter()
            .any(|event_slug| event_slug == slug)
        {
            return true;
        }

        self.prefix_selectors.iter().any(|selector| {
            selector
                .event_slug_prefix
                .as_deref()
                .is_some_and(|prefix| slug.starts_with(prefix))
        })
    }
}

pub struct PolymarketSelectorRefreshGuard {
    cancellation: CancellationToken,
    join_handle: JoinHandle<()>,
}

#[derive(Clone, Debug)]
pub struct PolymarketRulesetSetup {
    selectors: Vec<PolymarketRulesetSelector>,
    prefix_discoveries: Vec<PolymarketPrefixDiscovery>,
    selector_state: Option<PolymarketSelectorState>,
}

fn default_update_instruments_interval_mins() -> u64 {
    60
}

fn default_gamma_refresh_interval_secs() -> u64 {
    60
}

fn default_ws_max_subscriptions() -> usize {
    200
}

#[derive(Debug, Deserialize)]
pub struct PolymarketExecClientInput {
    pub account_id: String,
    pub signature_type: u8,
    pub funder: String,
}

fn map_signature_type(value: u8) -> Result<SignatureType, Box<dyn std::error::Error>> {
    match value {
        0 => Ok(SignatureType::Eoa),
        1 => Ok(SignatureType::PolyProxy),
        2 => Ok(SignatureType::PolyGnosisSafe),
        other => Err(format!("Unknown Polymarket signature_type: {other}").into()),
    }
}

pub fn build_data_client(
    raw: &Value,
    selectors: &[PolymarketRulesetSelector],
    selector_state: Option<PolymarketSelectorState>,
) -> Result<
    (
        Box<PolymarketDataClientFactory>,
        Box<PolymarketDataClientConfig>,
    ),
    Box<dyn std::error::Error>,
> {
    if !selectors.is_empty() && raw.get("event_slugs").is_some() {
        return Err(
            "data_clients[].config.event_slugs must be omitted when rulesets are enabled".into(),
        );
    }

    let common_input: PolymarketDataClientCommonInput = raw.clone().try_into()?;

    let filters: Vec<Arc<dyn InstrumentFilter>> = if selectors.is_empty() {
        let input: PolymarketDataClientInput = raw.clone().try_into()?;
        if input.event_slugs.is_empty() {
            vec![]
        } else {
            vec![Arc::new(EventSlugFilter::from_slugs(input.event_slugs))]
        }
    } else {
        let mut filters: Vec<Arc<dyn InstrumentFilter>> = selectors
            .iter()
            .filter(|selector| selector_state.is_none() || selector.event_slug_prefix.is_none())
            .map(|selector| {
                Arc::new(EventParamsFilter::new(GetGammaEventsParams {
                    tag_slug: Some(selector.tag_slug.clone()),
                    ..Default::default()
                })) as Arc<dyn InstrumentFilter>
            })
            .collect();

        if let Some(selector_state) = selector_state.clone() {
            filters.push(Arc::new(EventSlugFilter::new(move || {
                selector_state.current_event_slugs()
            })));
        }

        filters
    };

    let has_tag_only_selectors = selectors
        .iter()
        .any(|selector| selector.event_slug_prefix.is_none());
    let new_market_filter = if has_tag_only_selectors {
        None
    } else {
        selector_state.map(|selector_state| {
            Arc::new(NewMarketPredicateFilter::new(
                "ruleset-selector-prefix",
                move |new_market| selector_state.accepts_new_market_slug(&new_market.slug),
            )) as Arc<dyn InstrumentFilter>
        })
    };

    let config = PolymarketDataClientConfig {
        subscribe_new_markets: common_input.subscribe_new_markets,
        update_instruments_interval_mins: common_input.update_instruments_interval_mins,
        ws_max_subscriptions: common_input.ws_max_subscriptions,
        filters,
        new_market_filter,
        ..Default::default()
    };

    Ok((Box::new(PolymarketDataClientFactory), Box::new(config)))
}

pub fn polymarket_ruleset_tag_slugs(
    rulesets: &[RulesetConfig],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let selectors = polymarket_ruleset_selectors(rulesets)?;
    let mut tag_slugs = Vec::new();
    for selector in selectors {
        if !tag_slugs.contains(&selector.tag_slug) {
            tag_slugs.push(selector.tag_slug);
        }
    }

    Ok(tag_slugs)
}

fn polymarket_ruleset_selector(
    ruleset: &RulesetConfig,
) -> Result<PolymarketRulesetSelector, Box<dyn std::error::Error>> {
    let selector: PolymarketRulesetSelector =
        ruleset.selector.clone().try_into().map_err(|error| {
            format!(
                "failed to parse polymarket selector for ruleset {}: {error}",
                ruleset.id
            )
        })?;

    if selector.tag_slug.contains(char::is_whitespace) {
        return Err(format!(
            "polymarket selector tag_slug for ruleset {} must not contain whitespace, got {:?}",
            ruleset.id, selector.tag_slug
        )
        .into());
    }

    Ok(selector)
}

pub fn polymarket_ruleset_selectors(
    rulesets: &[RulesetConfig],
) -> Result<Vec<PolymarketRulesetSelector>, Box<dyn std::error::Error>> {
    let mut selectors = Vec::new();
    for ruleset in rulesets {
        if ruleset.venue != RulesetVenueKind::Polymarket {
            continue;
        }

        selectors.push(polymarket_ruleset_selector(ruleset)?);
    }

    Ok(selectors)
}

pub(crate) fn polymarket_ruleset_prefix_discoveries(
    rulesets: &[RulesetConfig],
) -> Result<Vec<PolymarketPrefixDiscovery>, Box<dyn std::error::Error>> {
    let mut discoveries = Vec::new();
    for ruleset in rulesets {
        if ruleset.venue != RulesetVenueKind::Polymarket {
            continue;
        }

        let selector = polymarket_ruleset_selector(ruleset)?;
        if selector.event_slug_prefix.is_none() {
            continue;
        }

        discoveries.push(PolymarketPrefixDiscovery {
            selector,
            min_time_to_expiry_secs: ruleset.min_time_to_expiry_secs,
            max_time_to_expiry_secs: ruleset.max_time_to_expiry_secs,
        });
    }

    Ok(discoveries)
}

pub(crate) fn polymarket_prefix_discovery_for_ruleset(
    ruleset: &RulesetConfig,
) -> Result<Option<PolymarketPrefixDiscovery>, Box<dyn std::error::Error>> {
    if ruleset.venue != RulesetVenueKind::Polymarket {
        return Ok(None);
    }

    let selector = polymarket_ruleset_selector(ruleset)?;
    if selector.event_slug_prefix.is_none() {
        return Ok(None);
    }

    Ok(Some(PolymarketPrefixDiscovery {
        selector,
        min_time_to_expiry_secs: ruleset.min_time_to_expiry_secs,
        max_time_to_expiry_secs: ruleset.max_time_to_expiry_secs,
    }))
}

pub fn gamma_refresh_interval_secs(raw: &Value) -> Result<u64, Box<dyn std::error::Error>> {
    let input: PolymarketDataClientCommonInput = raw.clone().try_into()?;
    Ok(input.gamma_refresh_interval_secs)
}

pub(crate) fn build_selector_state(
    prefix_discoveries: &[PolymarketPrefixDiscovery],
    timeout_secs: u64,
) -> Result<Option<PolymarketSelectorState>, Box<dyn std::error::Error>> {
    if prefix_discoveries.is_empty() {
        return Ok(None);
    }

    let event_slugs = resolve_event_slugs_for_prefix_discoveries(prefix_discoveries, timeout_secs)?;
    Ok(Some(PolymarketSelectorState::new(
        prefix_discoveries
            .iter()
            .map(|discovery| discovery.selector.clone())
            .collect(),
        event_slugs,
    )))
}

pub(crate) fn spawn_selector_refresh_task(
    selector_state: PolymarketSelectorState,
    prefix_discoveries: Vec<PolymarketPrefixDiscovery>,
    interval_secs: u64,
    timeout_secs: u64,
) -> Result<PolymarketSelectorRefreshGuard, Box<dyn std::error::Error>> {
    let raw_client = PolymarketGammaRawHttpClient::new(None, timeout_secs)
        .context("failed to build gamma raw client for selector refresh")?;
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let join_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = task_cancellation.cancelled() => return,
                _ = ticker.tick() => {}
            }

            match resolve_event_slugs_for_selectors_with_gamma_client(
                &prefix_discoveries,
                &raw_client,
            )
            .await
            {
                Ok(event_slugs) => selector_state.replace_event_slugs(event_slugs),
                Err(error) => {
                    log::warn!("failed refreshing polymarket selector event slugs: {error}");
                }
            }
        }
    });

    Ok(PolymarketSelectorRefreshGuard {
        cancellation,
        join_handle,
    })
}

impl PolymarketSelectorRefreshGuard {
    pub async fn shutdown(self) {
        self.cancellation.cancel();
        let _ = self.join_handle.await;
    }
}

impl PolymarketRulesetSetup {
    pub fn from_rulesets(
        rulesets: &[RulesetConfig],
        timeout_secs: u64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let selectors = polymarket_ruleset_selectors(rulesets)?;
        let prefix_discoveries = polymarket_ruleset_prefix_discoveries(rulesets)?;
        let selector_state = build_selector_state(&prefix_discoveries, timeout_secs)?;
        Ok(Self {
            selectors,
            prefix_discoveries,
            selector_state,
        })
    }

    pub fn build_data_client(
        &self,
        raw: &Value,
    ) -> Result<
        (
            Box<PolymarketDataClientFactory>,
            Box<PolymarketDataClientConfig>,
        ),
        Box<dyn std::error::Error>,
    > {
        build_data_client(raw, &self.selectors, self.selector_state.clone())
    }

    pub fn selector_state(&self) -> Option<PolymarketSelectorState> {
        self.selector_state.clone()
    }

    pub fn spawn_selector_refresh_task_if_configured(
        &self,
        raw: &Value,
        timeout_secs: u64,
    ) -> Result<Option<PolymarketSelectorRefreshGuard>, Box<dyn std::error::Error>> {
        let Some(selector_state) = self.selector_state.clone() else {
            return Ok(None);
        };

        spawn_selector_refresh_task(
            selector_state,
            self.prefix_discoveries.clone(),
            gamma_refresh_interval_secs(raw)?,
            timeout_secs,
        )
        .map(Some)
    }
}

fn gamma_selector_datetime_after_seconds(
    now: DateTime<Utc>,
    offset_secs: u64,
) -> anyhow::Result<String> {
    let offset_secs = i64::try_from(offset_secs)
        .map_err(|_| anyhow::anyhow!("selector expiry bound {offset_secs} exceeds i64::MAX"))?;
    Ok((now + ChronoDuration::seconds(offset_secs))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string())
}

pub(crate) fn gamma_event_params_for_prefix_discovery(
    discovery: &PolymarketPrefixDiscovery,
    now: DateTime<Utc>,
) -> anyhow::Result<GetGammaEventsParams> {
    Ok(GetGammaEventsParams {
        tag_slug: Some(discovery.selector.tag_slug.clone()),
        end_date_min: Some(gamma_selector_datetime_after_seconds(
            now,
            discovery.min_time_to_expiry_secs,
        )?),
        end_date_max: Some(gamma_selector_datetime_after_seconds(
            now,
            discovery.max_time_to_expiry_secs,
        )?),
        ..Default::default()
    })
}

pub(crate) fn resolve_event_slugs_for_prefix_discoveries(
    discoveries: &[PolymarketPrefixDiscovery],
    timeout_secs: u64,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let raw_client = PolymarketGammaRawHttpClient::new(None, timeout_secs)
        .context("failed to build gamma raw client")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(resolve_event_slugs_for_selectors_with_gamma_client(
        discoveries,
        &raw_client,
    ))
}

pub(crate) async fn resolve_matching_events_for_prefix_discoveries_with_gamma_client(
    discoveries: &[PolymarketPrefixDiscovery],
    client: &PolymarketGammaRawHttpClient,
) -> Result<Vec<GammaEvent>, Box<dyn std::error::Error>> {
    let mut matched_events = Vec::new();
    let mut seen_event_slugs: Vec<String> = Vec::new();
    let now = Utc::now();

    for discovery in discoveries {
        let params = gamma_event_params_for_prefix_discovery(discovery, now)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let events = fetch_gamma_events_paginated(client, params).await?;

        for event in events {
            let Some(event_slug) = event.slug.as_deref() else {
                continue;
            };
            if discovery
                .selector
                .event_slug_prefix
                .as_deref()
                .is_some_and(|prefix| event_slug.starts_with(prefix))
                && !seen_event_slugs.iter().any(|seen| seen == event_slug)
            {
                seen_event_slugs.push(event_slug.to_string());
                matched_events.push(event);
            }
        }
    }

    Ok(matched_events)
}

pub(crate) async fn resolve_event_slugs_for_selectors_with_gamma_client(
    discoveries: &[PolymarketPrefixDiscovery],
    client: &PolymarketGammaRawHttpClient,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(
        resolve_matching_events_for_prefix_discoveries_with_gamma_client(discoveries, client)
            .await?
            .into_iter()
            .filter_map(|event| event.slug)
            .collect(),
    )
}

pub(crate) async fn fetch_gamma_events_paginated(
    client: &PolymarketGammaRawHttpClient,
    base_params: GetGammaEventsParams,
) -> Result<Vec<GammaEvent>, Box<dyn std::error::Error>> {
    const PAGE_LIMIT: u32 = 100;

    let page_size = base_params.limit.unwrap_or(PAGE_LIMIT);
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

        if page_len < page_size {
            break;
        }

        offset += page_size;
    }

    Ok(all_events)
}

pub fn build_exec_client(
    raw: &Value,
    trader_id: TraderId,
    secrets: ResolvedPolymarketSecrets,
) -> Result<
    (
        Box<PolymarketExecutionClientFactory>,
        Box<PolymarketExecClientConfig>,
    ),
    Box<dyn std::error::Error>,
> {
    let input: PolymarketExecClientInput = raw.clone().try_into()?;

    let config = PolymarketExecClientConfig {
        trader_id,
        account_id: AccountId::from(input.account_id.as_str()),
        private_key: Some(secrets.private_key),
        api_key: Some(secrets.api_key),
        api_secret: Some(secrets.api_secret),
        passphrase: Some(secrets.passphrase),
        funder: Some(input.funder),
        signature_type: map_signature_type(input.signature_type)?,
        ..Default::default()
    };

    Ok((Box::new(PolymarketExecutionClientFactory), Box::new(config)))
}

pub fn build_fee_provider(
    raw: &Value,
    secrets: &ResolvedPolymarketSecrets,
    timeout_secs: u64,
) -> Result<Arc<dyn FeeProvider>, Box<dyn std::error::Error>> {
    let input: PolymarketExecClientInput = raw.clone().try_into()?;
    let secrets = PolymarketSecrets::resolve(
        Some(secrets.private_key.as_str()),
        Some(secrets.api_key.clone()),
        Some(secrets.api_secret.clone()),
        Some(secrets.passphrase.clone()),
        Some(input.funder),
    )
    .map_err(|error| format!("failed to resolve Polymarket fee credentials: {error}"))?;
    let client =
        PolymarketClobHttpClient::new(secrets.credential, secrets.address, None, timeout_secs)
            .map_err(|error| format!("failed to create Polymarket fee HTTP client: {error}"))?;

    Ok(Arc::new(PolymarketClobFeeProvider::new(client)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        collections::HashMap,
        net::SocketAddr,
        sync::{Arc, Mutex},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

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

    fn decode_query_param(value: &str) -> String {
        value.replace("%3A", ":").replace("%3a", ":")
    }

    fn prefix_discovery(
        tag_slug: &str,
        event_slug_prefix: &str,
        min_time_to_expiry_secs: u64,
        max_time_to_expiry_secs: u64,
    ) -> PolymarketPrefixDiscovery {
        PolymarketPrefixDiscovery {
            selector: PolymarketRulesetSelector {
                tag_slug: tag_slug.to_string(),
                event_slug_prefix: Some(event_slug_prefix.to_string()),
            },
            min_time_to_expiry_secs,
            max_time_to_expiry_secs,
        }
    }

    async fn spawn_test_server() -> (SocketAddr, Arc<Mutex<Vec<HashMap<String, String>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let server_requests = Arc::clone(&server_requests);
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let (path, params) = parse_request_target(&request);
                    server_requests.lock().unwrap().push(params.clone());

                    let body = if path == "/events"
                        && params.get("tag_slug").map(String::as_str) == Some("bitcoin")
                    {
                        json!([
                            {"id":"1","slug":"bitcoin-5m-alpha","markets":[]},
                            {"id":"2","slug":"bitcoin-15m-beta","markets":[]},
                            {"id":"3","slug":"ethereum-5m-gamma","markets":[]}
                        ])
                        .to_string()
                    } else {
                        "[]".to_string()
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        (addr, requests)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_event_slugs_for_selectors_with_gamma_client_filters_prefixes() {
        let (addr, requests) = spawn_test_server().await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let discoveries = vec![prefix_discovery("bitcoin", "bitcoin-5m", 30, 300)];

        let event_slugs =
            resolve_event_slugs_for_selectors_with_gamma_client(&discoveries, &client)
                .await
                .unwrap();

        assert_eq!(event_slugs, vec!["bitcoin-5m-alpha".to_string()]);

        let recorded_requests = requests.lock().unwrap();
        assert_eq!(recorded_requests.len(), 1);
        let params = &recorded_requests[0];
        assert_eq!(params.get("tag_slug").map(String::as_str), Some("bitcoin"));
        assert_eq!(params.get("active"), None);
        assert_eq!(params.get("closed"), None);
        assert_eq!(params.get("archived"), None);

        let end_date_min = DateTime::parse_from_rfc3339(&decode_query_param(
            params
                .get("end_date_min")
                .expect("prefix discovery should send end_date_min"),
        ))
        .unwrap();
        let end_date_max = DateTime::parse_from_rfc3339(&decode_query_param(
            params
                .get("end_date_max")
                .expect("prefix discovery should send end_date_max"),
        ))
        .unwrap();
        assert_eq!((end_date_max - end_date_min).num_seconds(), 270);
    }

    #[test]
    fn prefix_discovery_builds_narrowed_event_params() {
        let discovery = prefix_discovery("ethereum", "eth-updown-5m", 30, 300);
        let now = DateTime::parse_from_rfc3339("2026-04-16T10:38:38Z")
            .unwrap()
            .with_timezone(&Utc);

        let params = gamma_event_params_for_prefix_discovery(&discovery, now).unwrap();

        assert_eq!(params.tag_slug.as_deref(), Some("ethereum"));
        assert_eq!(params.active, None);
        assert_eq!(params.closed, None);
        assert_eq!(params.archived, None);
        assert_eq!(params.end_date_min.as_deref(), Some("2026-04-16T10:39:08Z"));
        assert_eq!(params.end_date_max.as_deref(), Some("2026-04-16T10:43:38Z"));
    }

    #[test]
    fn build_data_client_uses_event_slug_filter_when_selector_state_present() {
        let selectors = vec![PolymarketRulesetSelector {
            tag_slug: "bitcoin".to_string(),
            event_slug_prefix: Some("bitcoin-5m".to_string()),
        }];
        let selector_state = Some(PolymarketSelectorState::new(
            selectors.clone(),
            vec!["bitcoin-5m-alpha".to_string()],
        ));
        let raw = toml::toml! {
            subscribe_new_markets = true
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 45
            ws_max_subscriptions = 200
        }
        .into();

        let (_, config) = build_data_client(&raw, &selectors, selector_state).unwrap();
        let debug = format!("{config:?}");

        assert!(debug.contains("EventSlugFilter"), "{debug}");
        assert!(debug.contains("new_market_filter"), "{debug}");
    }

    #[test]
    fn build_data_client_uses_event_params_filter_for_tag_only_selectors() {
        let selectors = vec![PolymarketRulesetSelector {
            tag_slug: "bitcoin".to_string(),
            event_slug_prefix: None,
        }];
        let raw = toml::toml! {
            subscribe_new_markets = true
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 45
            ws_max_subscriptions = 200
        }
        .into();

        let (_, config) = build_data_client(&raw, &selectors, None).unwrap();
        let debug = format!("{config:?}");

        assert!(debug.contains("EventParamsFilter"), "{debug}");
        assert!(!debug.contains("EventSlugFilter"), "{debug}");
    }

    #[test]
    fn build_data_client_skips_new_market_filter_for_mixed_selectors() {
        let selectors = vec![
            PolymarketRulesetSelector {
                tag_slug: "bitcoin".to_string(),
                event_slug_prefix: Some("bitcoin-5m".to_string()),
            },
            PolymarketRulesetSelector {
                tag_slug: "ethereum".to_string(),
                event_slug_prefix: None,
            },
        ];
        let selector_state = Some(PolymarketSelectorState::new(
            vec![selectors[0].clone()],
            vec!["bitcoin-5m-alpha".to_string()],
        ));
        let raw = toml::toml! {
            subscribe_new_markets = true
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 45
            ws_max_subscriptions = 200
        }
        .into();

        let (_, config) = build_data_client(&raw, &selectors, selector_state).unwrap();
        let debug = format!("{config:?}");

        assert!(debug.contains("EventParamsFilter"), "{debug}");
        assert!(debug.contains("EventSlugFilter"), "{debug}");
        assert!(
            config.new_market_filter.is_none(),
            "mixed selectors should fail open for WS new-market discovery"
        );
    }

    #[test]
    fn selector_state_accepts_exact_and_prefix_new_market_slugs() {
        let selector_state = PolymarketSelectorState::new(
            vec![PolymarketRulesetSelector {
                tag_slug: "bitcoin".to_string(),
                event_slug_prefix: Some("bitcoin-5m".to_string()),
            }],
            vec!["bitcoin-1m-alpha".to_string()],
        );

        assert!(selector_state.accepts_new_market_slug("bitcoin-1m-alpha"));
        assert!(selector_state.accepts_new_market_slug("bitcoin-5m-beta"));
        assert!(!selector_state.accepts_new_market_slug("ethereum-5m-gamma"));
    }
}
