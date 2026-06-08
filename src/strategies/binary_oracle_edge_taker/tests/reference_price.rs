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
        ReferenceQuoteProvenance,
    },
};

const CHAINLINK_REFERENCE_PROVIDER: &str = "chainlink_ws";
const CHAINLINK_REFERENCE_INSTRUMENT: &str = "BTC-USD.CHAINLINK_REFERENCE";
const POLYRESEARCH_REFERENCE_PROVIDER: &str = "polyresearch_ws";
const POLYRESEARCH_REFERENCE_SYMBOL: &str = "BTC";
const CHAINLINK_PRIMARY_SOURCE_ID: &str = "chainlink_primary";
const POLYRESEARCH_BACKUP_SOURCE_ID: &str = "polyresearch_backup";
const TEST_REFERENCE_CURRENT_PRICE: f64 = 66_300.25;
const TEST_REFERENCE_OBSERVED_TS_MS: u64 = 1_200;
const TEST_REFERENCE_RECEIVED_TS_MS: u64 = 1_250;

fn reference_provider(key: &str) -> ReferencePriceProvider {
    ReferencePriceProvider::new(key).expect("test provider key should be valid")
}

fn reference_price_update(
    source_id: &str,
    provider: &str,
    provider_instrument: &str,
    price: f64,
    observed_ts_ms: u64,
    received_ts_ms: u64,
) -> nautilus_model::data::CustomData {
    ReferencePriceUpdate::try_new_with_provenance(
        "BTC",
        source_id,
        provider,
        provider_instrument,
        price,
        None,
        None,
        observed_ts_ms,
        received_ts_ms,
        ReferenceQuoteProvenance::empty(),
    )
    .expect("reference current price update should construct")
    .to_custom_data()
}

#[test]
fn custom_reference_price_update_does_not_mutate_price_to_beat() {
    let mut strategy = test_strategy();
    strategy.config.reference_current_price = Some(reference_price_config());
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);
    assert_eq!(strategy.active.price_to_beat, None);

    let update = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        TEST_REFERENCE_CURRENT_PRICE,
        TEST_REFERENCE_OBSERVED_TS_MS,
        TEST_REFERENCE_RECEIVED_TS_MS,
    );

    DataActor::on_data(&mut strategy, &update).expect("custom reference price should be handled");

    assert_eq!(
        strategy.active.reference_current_price,
        Some(TEST_REFERENCE_CURRENT_PRICE)
    );
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(CHAINLINK_PRIMARY_SOURCE_ID)
    );
    assert_eq!(
        strategy.active.reference_current_price_ts_ms,
        Some(TEST_REFERENCE_OBSERVED_TS_MS)
    );
    assert_eq!(strategy.active.price_to_beat, None);
}

#[test]
fn selected_reference_current_price_feeds_entry_pricing_spot() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.reference_current_price = Some(reference_price_config());
    strategy.active.last_reference_ts_ms = None;
    strategy.pricing.fast_spot = None;
    strategy.pricing.last_reference_current_price = None;
    let _cache = register_test_strategy(&mut strategy);

    let update = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        TEST_REFERENCE_CURRENT_PRICE,
        TEST_REFERENCE_OBSERVED_TS_MS,
        TEST_REFERENCE_RECEIVED_TS_MS,
    );

    DataActor::on_data(&mut strategy, &update)
        .expect("custom reference current price should be handled");

    let inputs = strategy
        .current_entry_pricing_inputs_at(TEST_REFERENCE_OBSERVED_TS_MS)
        .expect("selected reference current price should supply entry spot");
    assert_eq!(inputs.spot_price, TEST_REFERENCE_CURRENT_PRICE);
    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot(
            CHAINLINK_PRIMARY_SOURCE_ID,
            TEST_REFERENCE_CURRENT_PRICE,
            TEST_REFERENCE_OBSERVED_TS_MS
        ))
    );
}

#[test]
fn reference_price_sources_subscribe_as_custom_data_on_start_and_unsubscribe_on_stop() {
    let mut strategy = test_strategy();
    strategy.config.reference_current_price = Some(reference_price_config());
    register_test_strategy(&mut strategy);

    DataActor::on_start(&mut strategy).expect("strategy should start");

    assert_eq!(strategy.reference_price_subscribe_events.len(), 2);
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[0],
        REFERENCE_PRICE_SUBSCRIBE_ACTION,
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BTC-USD.CHAINLINK_REFERENCE"),
        None,
    );
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[1],
        REFERENCE_PRICE_SUBSCRIBE_ACTION,
        POLYRESEARCH_BACKUP_SOURCE_ID,
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
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BTC-USD.CHAINLINK_REFERENCE"),
        None,
    );
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[3],
        REFERENCE_PRICE_UNSUBSCRIBE_ACTION,
        POLYRESEARCH_BACKUP_SOURCE_ID,
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
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        TEST_REFERENCE_CURRENT_PRICE,
        1_100,
        1_110,
    );
    DataActor::on_data(&mut strategy, &backup).expect("backup quote should be handled");

    assert_eq!(
        strategy.active.reference_current_price,
        Some(TEST_REFERENCE_CURRENT_PRICE)
    );
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(POLYRESEARCH_BACKUP_SOURCE_ID)
    );

    let later_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        66_500.00,
        1_120,
        1_125,
    );
    DataActor::on_data(&mut strategy, &later_primary).expect("primary quote should be handled");

    assert_eq!(
        strategy.active.reference_current_price,
        Some(TEST_REFERENCE_CURRENT_PRICE)
    );
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(POLYRESEARCH_BACKUP_SOURCE_ID)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Available)
    );

    let backup_update = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        66_301.25,
        1_130,
        1_135,
    );
    DataActor::on_data(&mut strategy, &backup_update)
        .expect("winning backup quote should be handled");

    assert_eq!(strategy.active.reference_current_price, Some(66_301.25));
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(POLYRESEARCH_BACKUP_SOURCE_ID)
    );
}

#[test]
fn reference_price_interval_transition_clears_stale_quotes_and_health() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let first_interval_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        TEST_REFERENCE_CURRENT_PRICE,
        1_100,
        1_110,
    );
    DataActor::on_data(&mut strategy, &first_interval_primary)
        .expect("first interval quote should be handled");

    assert!(
        strategy
            .reference_price_quotes
            .contains_key(CHAINLINK_PRIMARY_SOURCE_ID)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Available)
    );

    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-2", 1_200), 0);
    let second_interval_backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        66_500.25,
        1_200,
        1_210,
    );
    DataActor::on_data(&mut strategy, &second_interval_backup)
        .expect("second interval quote should be handled");

    assert!(
        !strategy
            .reference_price_quotes
            .contains_key(CHAINLINK_PRIMARY_SOURCE_ID)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Silent)
    );
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(POLYRESEARCH_BACKUP_SOURCE_ID)
    );
}

#[test]
fn stale_reference_price_replay_does_not_evict_fresh_same_interval_quotes() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price.min_valid_sources = 2;
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let fresh_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        TEST_REFERENCE_CURRENT_PRICE,
        1_100,
        1_110,
    );
    DataActor::on_data(&mut strategy, &fresh_primary)
        .expect("fresh primary quote should be handled");

    let fresh_backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        66_301.00,
        1_105,
        1_115,
    );
    DataActor::on_data(&mut strategy, &fresh_backup).expect("fresh backup quote should be handled");

    let stale_primary_replay = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        66_100.00,
        900,
        1_120,
    );
    DataActor::on_data(&mut strategy, &stale_primary_replay)
        .expect("stale primary replay should be handled fail-closed");

    let primary_quote = strategy
        .reference_price_quotes
        .get(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("fresh primary quote should survive stale replay");
    assert_eq!(primary_quote.observed_ts_ms(), 1_100);

    let fresh_backup_update = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        66_301.25,
        1_125,
        1_130,
    );
    DataActor::on_data(&mut strategy, &fresh_backup_update)
        .expect("fresh backup update should be handled after stale replay");

    assert!(
        strategy
            .reference_price_quotes
            .contains_key(CHAINLINK_PRIMARY_SOURCE_ID),
        "fresh primary quote must not be cleared by stale replay cleanup"
    );
    assert_eq!(
        strategy
            .reference_price_quotes
            .get(POLYRESEARCH_BACKUP_SOURCE_ID)
            .map(|quote| quote.observed_ts_ms()),
        Some(1_125)
    );
    assert_eq!(
        strategy.active.reference_current_price,
        Some(TEST_REFERENCE_CURRENT_PRICE)
    );
}

#[test]
fn reference_price_update_with_wrong_provider_does_not_satisfy_source() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let wrong_provider = ReferencePriceUpdate::try_new(
        "BTC",
        CHAINLINK_PRIMARY_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        "BTC/USD",
        TEST_REFERENCE_CURRENT_PRICE,
        None,
        None,
        1_100,
        1_110,
    )
    .expect("wrong-provider update should construct")
    .to_custom_data();

    DataActor::on_data(&mut strategy, &wrong_provider)
        .expect("wrong-provider custom data should be handled fail-closed");

    assert_eq!(strategy.active.reference_current_price, None);
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::MalformedFrame)
    );
}

#[test]
fn reference_price_update_with_wrong_provider_instrument_does_not_satisfy_source() {
    let mut strategy = test_strategy();
    strategy.config.reference_current_price = Some(reference_price_config());
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let wrong_instrument = ReferencePriceUpdate::try_new_with_provenance(
        "BTC",
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        "ETH-USD.CHAINLINK_REFERENCE",
        TEST_REFERENCE_CURRENT_PRICE,
        None,
        None,
        1_100,
        1_110,
        ReferenceQuoteProvenance::empty(),
    )
    .expect("wrong-instrument update should construct")
    .to_custom_data();

    DataActor::on_data(&mut strategy, &wrong_instrument)
        .expect("wrong-instrument custom data should be handled fail-closed");

    assert_eq!(strategy.active.reference_current_price, None);
    assert_eq!(strategy.pricing.last_reference_current_price, None);
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::MalformedFrame)
    );
}

#[test]
fn stale_block_marks_reference_price_source_stale() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    reference_price.max_source_age_ms = 50;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let stale_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_100,
        1_110,
    );
    DataActor::on_data(&mut strategy, &stale_primary).expect("stale quote should be handled");

    assert_eq!(strategy.active.reference_current_price, None);
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Stale)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(POLYRESEARCH_BACKUP_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Silent)
    );
}

#[test]
fn drift_block_marks_reference_price_sources_unavailable() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    reference_price.min_valid_sources = 2;
    reference_price.max_source_drift_bps = 50;
    reference_price.drift_policy = ReferencePriceDriftPolicy::Block;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_100,
        1_110,
    );
    DataActor::on_data(&mut strategy, &primary).expect("primary quote should be handled");

    let backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        101.0,
        1_100,
        1_110,
    );
    DataActor::on_data(&mut strategy, &backup).expect("backup quote should be handled");

    assert_eq!(strategy.active.reference_current_price, None);
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::DriftExceeded)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(POLYRESEARCH_BACKUP_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::DriftExceeded)
    );
}

fn reference_price_config() -> ReferencePriceBlock {
    let mut sources = BTreeMap::new();
    sources.insert(
        CHAINLINK_PRIMARY_SOURCE_ID.to_string(),
        ReferencePriceSourceBlock {
            provider: reference_provider(CHAINLINK_REFERENCE_PROVIDER),
            enabled: true,
            required: true,
            client_id: ClientId::from("chainlink_reference"),
            instrument_id: Some(CHAINLINK_REFERENCE_INSTRUMENT.to_string()),
            symbol: None,
        },
    );
    sources.insert(
        POLYRESEARCH_BACKUP_SOURCE_ID.to_string(),
        ReferencePriceSourceBlock {
            provider: reference_provider(POLYRESEARCH_REFERENCE_PROVIDER),
            enabled: true,
            required: false,
            client_id: ClientId::from("polyresearch_reference"),
            instrument_id: None,
            symbol: Some(POLYRESEARCH_REFERENCE_SYMBOL.to_string()),
        },
    );
    ReferencePriceBlock {
        asset: "BTC".to_string(),
        source_order: vec![
            CHAINLINK_PRIMARY_SOURCE_ID.to_string(),
            POLYRESEARCH_BACKUP_SOURCE_ID.to_string(),
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
