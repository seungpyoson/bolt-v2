use std::sync::Arc;

use nautilus_polymarket::http::query::GetGammaEventsParams;
use nautilus_model::identifiers::{AccountId, TraderId};
use nautilus_polymarket::{
    common::enums::SignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    factories::{PolymarketDataClientFactory, PolymarketExecutionClientFactory},
    filters::{EventParamsFilter, EventSlugFilter, InstrumentFilter},
};
use serde::Deserialize;
use toml::Value;

use crate::config::{RulesetConfig, RulesetVenueKind};
use crate::secrets::ResolvedPolymarketSecrets;

#[derive(Debug, Deserialize)]
pub struct PolymarketDataClientInput {
    #[serde(default)]
    pub subscribe_new_markets: bool,
    #[serde(default = "default_update_instruments_interval_mins")]
    pub update_instruments_interval_mins: u64,
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
    #[serde(default = "default_ws_max_subscriptions")]
    ws_max_subscriptions: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct PolymarketSelector {
    tag_slug: String,
    #[allow(dead_code)]
    #[serde(default)]
    event_slug_prefix: Option<String>,
}

fn default_update_instruments_interval_mins() -> u64 {
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
    selector_tag_slugs: &[String],
) -> Result<
    (
        Box<PolymarketDataClientFactory>,
        Box<PolymarketDataClientConfig>,
    ),
    Box<dyn std::error::Error>,
> {
    let common_input: PolymarketDataClientCommonInput = raw.clone().try_into()?;

    let filters: Vec<Arc<dyn InstrumentFilter>> = if selector_tag_slugs.is_empty() {
        let input: PolymarketDataClientInput = raw.clone().try_into()?;
        if input.event_slugs.is_empty() {
            vec![]
        } else {
            vec![Arc::new(EventSlugFilter::from_slugs(input.event_slugs))]
        }
    } else {
        selector_tag_slugs
            .iter()
            .cloned()
            .map(|tag_slug| {
                Arc::new(EventParamsFilter::new(GetGammaEventsParams {
                    tag_slug: Some(tag_slug),
                    ..Default::default()
                })) as Arc<dyn InstrumentFilter>
            })
            .collect()
    };

    let config = PolymarketDataClientConfig {
        subscribe_new_markets: common_input.subscribe_new_markets,
        update_instruments_interval_mins: common_input.update_instruments_interval_mins,
        ws_max_subscriptions: common_input.ws_max_subscriptions,
        filters,
        ..Default::default()
    };

    Ok((Box::new(PolymarketDataClientFactory), Box::new(config)))
}

pub fn polymarket_ruleset_tag_slugs(
    rulesets: &[RulesetConfig],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut tag_slugs = Vec::new();
    for ruleset in rulesets {
        if ruleset.venue != RulesetVenueKind::Polymarket {
            continue;
        }

        let selector: PolymarketSelector = ruleset
            .selector
            .clone()
            .try_into()
            .map_err(|error| {
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

        if !tag_slugs.contains(&selector.tag_slug) {
            tag_slugs.push(selector.tag_slug);
        }
    }

    Ok(tag_slugs)
}

pub fn polymarket_ruleset_selectors(
    rulesets: &[RulesetConfig],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    polymarket_ruleset_tag_slugs(rulesets)
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
