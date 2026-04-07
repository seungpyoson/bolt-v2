use std::sync::Arc;

use nautilus_model::identifiers::{AccountId, TraderId};
use nautilus_polymarket::{
    common::enums::SignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    factories::{PolymarketDataClientFactory, PolymarketExecutionClientFactory},
    filters::{EventSlugFilter, InstrumentFilter},
};
use serde::Deserialize;
use toml::Value;

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
) -> Result<
    (
        Box<PolymarketDataClientFactory>,
        Box<PolymarketDataClientConfig>,
    ),
    Box<dyn std::error::Error>,
> {
    let input: PolymarketDataClientInput = raw.clone().try_into()?;
    let filters: Vec<Arc<dyn InstrumentFilter>> = if input.event_slugs.is_empty() {
        vec![]
    } else {
        vec![Arc::new(EventSlugFilter::from_slugs(input.event_slugs))]
    };

    let config = PolymarketDataClientConfig {
        subscribe_new_markets: input.subscribe_new_markets,
        update_instruments_interval_mins: input.update_instruments_interval_mins,
        ws_max_subscriptions: input.ws_max_subscriptions,
        filters,
        ..Default::default()
    };

    Ok((Box::new(PolymarketDataClientFactory), Box::new(config)))
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
