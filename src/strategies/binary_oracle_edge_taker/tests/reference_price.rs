#![cfg(test)]

use std::collections::BTreeMap;

use super::*;
use crate::{
    bolt_v3_config::{
        ReferencePriceBlock, ReferencePriceDriftPolicy, ReferencePriceProvider,
        ReferencePriceSelectionPolicy, ReferencePriceSourceBlock, ReferencePriceStalePolicy,
    },
    bolt_v3_reference_price::{
        REFERENCE_PRICE_ASSET_PARAM, REFERENCE_PRICE_INSTRUMENT_ID_PARAM,
        REFERENCE_PRICE_PROVIDER_PARAM, REFERENCE_PRICE_SOURCE_KEY_PARAM,
        REFERENCE_PRICE_SYMBOL_PARAM, ReferencePriceSourceStatus, ReferencePriceUpdate,
    },
};

const CHAINLINK_REFERENCE_PROVIDER: &str = "chainlink_ws";
const POLYRESEARCH_REFERENCE_PROVIDER: &str = "polyresearch_ws";

fn reference_provider(key: &str) -> ReferencePriceProvider {
    ReferencePriceProvider::new(key).expect("test provider key should be valid")
}

#[test]
fn custom_reference_price_update_does_not_mutate_price_to_beat() {
    let mut strategy = test_strategy();
    strategy.config.reference_price = Some(reference_price_config());
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);
    assert_eq!(strategy.active.price_to_beat, None);

    let update = ReferencePriceUpdate::try_new(
        "BTC",
        "chainlink_primary",
        "chainlink_ws",
        66_300.25,
        None,
        None,
        1_200,
        1_250,
    )
    .expect("reference price update should construct")
    .to_custom_data();

    DataActor::on_data(&mut strategy, &update).expect("custom reference price should be handled");

    assert_eq!(strategy.active.reference_price, Some(66_300.25));
    assert_eq!(
        strategy.active.reference_price_source_id.as_deref(),
        Some("chainlink_primary")
    );
    assert_eq!(strategy.active.reference_price_ts_ms, Some(1_200));
    assert_eq!(strategy.active.price_to_beat, None);
}

#[test]
fn reference_price_sources_subscribe_as_custom_data_on_start_and_unsubscribe_on_stop() {
    let mut strategy = test_strategy();
    strategy.config.reference_price = Some(reference_price_config());
    register_test_strategy(&mut strategy);

    DataActor::on_start(&mut strategy).expect("strategy should start");

    assert_eq!(strategy.reference_price_subscribe_events.len(), 2);
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[0],
        REFERENCE_PRICE_SUBSCRIBE_ACTION,
        "chainlink_primary",
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BTC-USD.CHAINLINK_REFERENCE"),
        None,
    );
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[1],
        REFERENCE_PRICE_SUBSCRIBE_ACTION,
        "polyresearch_backup",
        POLYRESEARCH_REFERENCE_PROVIDER,
        "polyresearch_reference",
        None,
        Some("BTC"),
    );

    DataActor::on_stop(&mut strategy).expect("strategy should stop");

    assert_eq!(strategy.reference_price_subscribe_events.len(), 4);
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[2],
        REFERENCE_PRICE_UNSUBSCRIBE_ACTION,
        "chainlink_primary",
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BTC-USD.CHAINLINK_REFERENCE"),
        None,
    );
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[3],
        REFERENCE_PRICE_UNSUBSCRIBE_ACTION,
        "polyresearch_backup",
        POLYRESEARCH_REFERENCE_PROVIDER,
        "polyresearch_reference",
        None,
        Some("BTC"),
    );
}

#[test]
fn non_winning_reference_price_source_updates_health_without_trading_state() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut("chainlink_primary")
        .expect("chainlink source should exist")
        .required = false;
    strategy.config.reference_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let backup = ReferencePriceUpdate::try_new(
        "BTC",
        "polyresearch_backup",
        POLYRESEARCH_REFERENCE_PROVIDER,
        66_300.25,
        None,
        None,
        1_100,
        1_110,
    )
    .expect("backup quote should construct")
    .to_custom_data();
    DataActor::on_data(&mut strategy, &backup).expect("backup quote should be handled");

    assert_eq!(strategy.active.reference_price, Some(66_300.25));
    assert_eq!(
        strategy.active.reference_price_source_id.as_deref(),
        Some("polyresearch_backup")
    );

    let later_primary = ReferencePriceUpdate::try_new(
        "BTC",
        "chainlink_primary",
        CHAINLINK_REFERENCE_PROVIDER,
        66_500.00,
        None,
        None,
        1_120,
        1_125,
    )
    .expect("primary quote should construct")
    .to_custom_data();
    DataActor::on_data(&mut strategy, &later_primary).expect("primary quote should be handled");

    assert_eq!(strategy.active.reference_price, Some(66_300.25));
    assert_eq!(
        strategy.active.reference_price_source_id.as_deref(),
        Some("polyresearch_backup")
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get("chainlink_primary")
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Available)
    );

    let backup_update = ReferencePriceUpdate::try_new(
        "BTC",
        "polyresearch_backup",
        POLYRESEARCH_REFERENCE_PROVIDER,
        66_301.25,
        None,
        None,
        1_130,
        1_135,
    )
    .expect("backup update should construct")
    .to_custom_data();
    DataActor::on_data(&mut strategy, &backup_update)
        .expect("winning backup quote should be handled");

    assert_eq!(strategy.active.reference_price, Some(66_301.25));
    assert_eq!(
        strategy.active.reference_price_source_id.as_deref(),
        Some("polyresearch_backup")
    );
}

#[test]
fn reference_price_interval_transition_clears_stale_quotes_and_health() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut("chainlink_primary")
        .expect("chainlink source should exist")
        .required = false;
    strategy.config.reference_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let first_interval_primary = ReferencePriceUpdate::try_new(
        "BTC",
        "chainlink_primary",
        CHAINLINK_REFERENCE_PROVIDER,
        66_300.25,
        None,
        None,
        1_100,
        1_110,
    )
    .expect("primary quote should construct")
    .to_custom_data();
    DataActor::on_data(&mut strategy, &first_interval_primary)
        .expect("first interval quote should be handled");

    assert!(
        strategy
            .reference_price_quotes
            .contains_key("chainlink_primary")
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get("chainlink_primary")
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Available)
    );

    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-2", 1_200), 0);
    let second_interval_backup = ReferencePriceUpdate::try_new(
        "BTC",
        "polyresearch_backup",
        POLYRESEARCH_REFERENCE_PROVIDER,
        66_500.25,
        None,
        None,
        1_200,
        1_210,
    )
    .expect("backup quote should construct")
    .to_custom_data();
    DataActor::on_data(&mut strategy, &second_interval_backup)
        .expect("second interval quote should be handled");

    assert!(
        !strategy
            .reference_price_quotes
            .contains_key("chainlink_primary")
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get("chainlink_primary")
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Silent)
    );
    assert_eq!(
        strategy.active.reference_price_source_id.as_deref(),
        Some("polyresearch_backup")
    );
}

fn reference_price_config() -> ReferencePriceBlock {
    let mut sources = BTreeMap::new();
    sources.insert(
        "chainlink_primary".to_string(),
        ReferencePriceSourceBlock {
            provider: reference_provider(CHAINLINK_REFERENCE_PROVIDER),
            enabled: true,
            required: true,
            client_id: ClientId::from("chainlink_reference"),
            instrument_id: Some("BTC-USD.CHAINLINK_REFERENCE".to_string()),
            symbol: None,
        },
    );
    sources.insert(
        "polyresearch_backup".to_string(),
        ReferencePriceSourceBlock {
            provider: reference_provider(POLYRESEARCH_REFERENCE_PROVIDER),
            enabled: true,
            required: false,
            client_id: ClientId::from("polyresearch_reference"),
            instrument_id: None,
            symbol: Some("BTC".to_string()),
        },
    );
    ReferencePriceBlock {
        asset: "BTC".to_string(),
        source_order: vec![
            "chainlink_primary".to_string(),
            "polyresearch_backup".to_string(),
        ],
        min_valid_sources: 1,
        selection_policy: ReferencePriceSelectionPolicy::FirstValidPerInterval,
        max_source_age_ms: 1_000,
        max_source_drift_bps: 50,
        drift_policy: ReferencePriceDriftPolicy::Observe,
        stale_policy: ReferencePriceStalePolicy::Block,
        sources,
    }
}

fn assert_reference_price_subscription(
    event: &ReferencePriceSubscribeEvent,
    action: &str,
    source_id: &str,
    provider: &str,
    client_id: &str,
    instrument_id: Option<&str>,
    symbol: Option<&str>,
) {
    assert_eq!(event.action, action);
    assert_eq!(event.source_id, source_id);
    assert_eq!(event.provider, provider);
    assert_eq!(event.client_id, ClientId::from(client_id));
    assert_eq!(event.data_type.type_name(), "BoltV3ReferencePriceUpdate");
    assert_eq!(event.data_type.identifier(), Some("BTC"));
    let metadata = event
        .data_type
        .metadata()
        .expect("reference price data type should carry metadata");
    assert_eq!(metadata.get_str(REFERENCE_PRICE_ASSET_PARAM), Some("BTC"));
    assert_eq!(
        metadata.get_str(REFERENCE_PRICE_SOURCE_KEY_PARAM),
        Some(source_id)
    );
    assert_eq!(
        metadata.get_str(REFERENCE_PRICE_PROVIDER_PARAM),
        Some(provider)
    );
    assert_eq!(
        event.params.get_str(REFERENCE_PRICE_SOURCE_KEY_PARAM),
        Some(source_id)
    );
    assert_eq!(
        event.params.get_str(REFERENCE_PRICE_PROVIDER_PARAM),
        Some(provider)
    );
    assert_eq!(
        event.params.get_str(REFERENCE_PRICE_INSTRUMENT_ID_PARAM),
        instrument_id
    );
    assert_eq!(event.params.get_str(REFERENCE_PRICE_SYMBOL_PARAM), symbol);
}
