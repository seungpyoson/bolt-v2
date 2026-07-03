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

#[test]
fn signal_quote_subscription_uses_configured_signal_client() {
    let strategy = test_strategy();

    assert_eq!(
        strategy.signal_instrument_id(),
        Some(InstrumentId::from("SIGNAL.SOURCE"))
    );
    assert_eq!(
        strategy.signal_client_id(),
        Some(ClientId::from("signal_data_client"))
    );
}

#[test]
fn signal_quote_subscription_without_signal_client_preserves_default_routing() {
    let mut strategy = test_strategy();
    strategy.config.signal_venue = None;

    assert_eq!(
        strategy.signal_instrument_id(),
        Some(InstrumentId::from("SIGNAL.SOURCE"))
    );
    assert_eq!(strategy.signal_client_id(), None);
}

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
    strategy.pricing.set_selected_pricing_spot(None);
    strategy.pricing.set_last_reference_observation(None, None);
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
        strategy.pricing.selected_pricing_spot().cloned(),
        Some(fast_spot_received(
            CHAINLINK_PRIMARY_SOURCE_ID,
            TEST_REFERENCE_CURRENT_PRICE,
            TEST_REFERENCE_OBSERVED_TS_MS,
            Some(TEST_REFERENCE_RECEIVED_TS_MS),
        ))
    );
}

#[test]
fn active_interval_rollover_clears_reference_current_price_pricing_state() {
    let mut strategy = test_strategy();
    strategy.config.reference_current_price = Some(reference_price_config());
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
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
        .expect("first interval reference current price should be handled");

    assert_eq!(
        strategy.pricing.last_reference_current_price(),
        Some(TEST_REFERENCE_CURRENT_PRICE)
    );
    assert_eq!(
        strategy.pricing.selected_pricing_spot().cloned(),
        Some(fast_spot_received(
            CHAINLINK_PRIMARY_SOURCE_ID,
            TEST_REFERENCE_CURRENT_PRICE,
            TEST_REFERENCE_OBSERVED_TS_MS,
            Some(TEST_REFERENCE_RECEIVED_TS_MS),
        ))
    );

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_400));

    assert_eq!(strategy.active.reference_current_price, None);
    assert_eq!(strategy.active.reference_current_price_source_id, None);
    assert_eq!(strategy.active.reference_current_price_ts_ms, None);
    assert_eq!(strategy.active.last_reference_ts_ms, None);
    assert_eq!(strategy.pricing.last_reference_current_price(), None);
    assert_eq!(strategy.pricing.last_reference_current_price_ts_ms(), None);
    assert_eq!(strategy.pricing.selected_pricing_spot().cloned(), None);
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
fn selection_retry_reissues_missing_live_input_subscriptions() {
    let mut strategy = test_strategy();
    strategy.config.reference_current_price = Some(reference_price_config());

    strategy.retry_missing_live_input_subscriptions_at(1_500);

    assert_eq!(
        strategy.live_input_subscription_retry_events,
        vec![LiveInputSubscriptionRetryEvent {
            signal_missing: true,
            reference_missing: true,
            realized_volatility_missing: true,
        }]
    );
    assert_eq!(strategy.reference_price_subscribe_events.len(), 4);
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[0],
        REFERENCE_PRICE_UNSUBSCRIBE_ACTION,
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BTC-USD.CHAINLINK_REFERENCE"),
        None,
    );
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[2],
        REFERENCE_PRICE_SUBSCRIBE_ACTION,
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BTC-USD.CHAINLINK_REFERENCE"),
        None,
    );
}

#[test]
fn selection_retry_reissues_frozen_reference_stream_with_stale_buffered_quote() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(POLYRESEARCH_BACKUP_SOURCE_ID)
        .expect("polyresearch source should exist")
        .enabled = false;
    reference_price.max_source_age_ms = 100;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_125_u64 * NANOS_PER_MILLI_U64));
    let initial_quote = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_100,
        1_105,
    );
    DataActor::on_data(&mut strategy, &initial_quote)
        .expect("initial reference quote should be handled");

    assert!(
        strategy
            .reference_price_quotes
            .contains_key(CHAINLINK_PRIMARY_SOURCE_ID),
        "precondition: the stale quote remains buffered"
    );
    assert_eq!(strategy.active.reference_current_price, Some(100.0));

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_250_u64 * NANOS_PER_MILLI_U64));
    let mut retry_snapshot = active_snapshot_with_start("MKT-1", 1_000);
    retry_snapshot.published_at_ms = 1_250;
    strategy.apply_selection_snapshot(retry_snapshot);

    assert!(
        strategy
            .reference_price_quotes
            .contains_key(CHAINLINK_PRIMARY_SOURCE_ID),
        "the buffered quote must not be dropped just to force a retry"
    );
    assert_eq!(strategy.active.reference_current_price, None);
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Stale)
    );

    let retry_event_count = strategy.live_input_subscription_retry_events.len();
    let subscribe_event_count = strategy.reference_price_subscribe_events.len();
    strategy.retry_missing_live_input_subscriptions_at(1_250);

    assert_eq!(
        strategy.live_input_subscription_retry_events[retry_event_count].reference_missing,
        true
    );
    assert_eq!(
        strategy.reference_price_subscribe_events.len(),
        subscribe_event_count + 2,
        "stale buffered reference quote must not suppress unsubscribe/subscribe recovery"
    );
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[subscribe_event_count],
        REFERENCE_PRICE_UNSUBSCRIBE_ACTION,
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BTC-USD.CHAINLINK_REFERENCE"),
        None,
    );
    assert_reference_price_subscription(
        &strategy.reference_price_subscribe_events[subscribe_event_count + 1],
        REFERENCE_PRICE_SUBSCRIBE_ACTION,
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BTC-USD.CHAINLINK_REFERENCE"),
        None,
    );

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_275_u64 * NANOS_PER_MILLI_U64));
    let recovered_quote = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        101.0,
        1_270,
        1_272,
    );
    DataActor::on_data(&mut strategy, &recovered_quote)
        .expect("recovered reference quote should be handled");

    assert_eq!(strategy.active.reference_current_price, Some(101.0));
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(CHAINLINK_PRIMARY_SOURCE_ID)
    );
    assert_eq!(strategy.pricing.last_reference_current_price(), Some(101.0));
}

#[test]
fn configured_polyresearch_reference_price_source_subscribes_for_asset() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price.asset = "BNB".to_string();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .instrument_id = Some("BNB-USD.CHAINLINK_REFERENCE".to_string());
    reference_price
        .sources
        .get_mut(POLYRESEARCH_BACKUP_SOURCE_ID)
        .expect("polyresearch source should exist")
        .symbol = Some("BNB".to_string());
    strategy.config.reference_current_price = Some(reference_price);
    register_test_strategy(&mut strategy);

    DataActor::on_start(&mut strategy).expect("strategy should start");

    assert_eq!(strategy.reference_price_subscribe_events.len(), 2);
    assert_reference_price_subscription_for_asset(
        &strategy.reference_price_subscribe_events[0],
        "BNB",
        REFERENCE_PRICE_SUBSCRIBE_ACTION,
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BNB-USD.CHAINLINK_REFERENCE"),
        None,
    );
    assert_reference_price_subscription_for_asset(
        &strategy.reference_price_subscribe_events[1],
        "BNB",
        REFERENCE_PRICE_SUBSCRIBE_ACTION,
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        "polyresearch_reference",
        None,
        Some("BNB"),
    );

    DataActor::on_stop(&mut strategy).expect("strategy should stop");

    assert_eq!(strategy.reference_price_subscribe_events.len(), 4);
    assert_reference_price_subscription_for_asset(
        &strategy.reference_price_subscribe_events[2],
        "BNB",
        REFERENCE_PRICE_UNSUBSCRIBE_ACTION,
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        "chainlink_reference",
        Some("BNB-USD.CHAINLINK_REFERENCE"),
        None,
    );
    assert_reference_price_subscription_for_asset(
        &strategy.reference_price_subscribe_events[3],
        "BNB",
        REFERENCE_PRICE_UNSUBSCRIBE_ACTION,
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        "polyresearch_reference",
        None,
        Some("BNB"),
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
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
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
fn selected_backup_with_older_timestamp_replaces_previous_source() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    let _cache = register_test_strategy(&mut strategy);

    let primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_200,
        1_205,
    );
    DataActor::on_data(&mut strategy, &primary).expect("primary quote should be handled");
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(CHAINLINK_PRIMARY_SOURCE_ID)
    );

    let backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        101.0,
        1_150,
        1_155,
    );
    DataActor::on_data(&mut strategy, &backup).expect("backup quote should be handled");
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(CHAINLINK_PRIMARY_SOURCE_ID),
        "selector should keep the current source while it remains valid"
    );

    strategy
        .reference_price_quotes
        .remove(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("primary quote should be present before simulated source loss");
    strategy.observe_current_reference_price_selection(
        strategy
            .active
            .interval_start_ms
            .expect("test market should be interval-bound"),
        strategy
            .active
            .interval_end_ms
            .expect("test market should be interval-bound"),
        1_250,
    );

    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(POLYRESEARCH_BACKUP_SOURCE_ID)
    );
    assert_eq!(strategy.active.reference_current_price, Some(101.0));
    assert_eq!(strategy.active.reference_current_price_ts_ms, Some(1_150));
    assert_eq!(strategy.pricing.last_reference_current_price(), Some(101.0));
    assert_eq!(
        strategy.pricing.last_reference_current_price_source_id(),
        Some(POLYRESEARCH_BACKUP_SOURCE_ID)
    );
    assert_eq!(
        strategy.pricing.last_reference_current_price_ts_ms(),
        Some(1_150)
    );
}

#[test]
fn selection_retry_refreshes_reference_failover_before_forced_flat_check() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    reference_price.max_source_age_ms = 100;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.config.forced_flat_stale_reference_ms = 100;
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_120_u64 * NANOS_PER_MILLI_U64));
    let primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_100,
        1_105,
    );
    DataActor::on_data(&mut strategy, &primary).expect("primary quote should be handled");

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_140_u64 * NANOS_PER_MILLI_U64));
    let backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        101.0,
        1_130,
        1_135,
    );
    DataActor::on_data(&mut strategy, &backup)
        .expect("backup quote should be held while primary is still valid");
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(CHAINLINK_PRIMARY_SOURCE_ID)
    );
    assert!(
        strategy
            .active_forced_flat_reasons_at(1_220)
            .contains(&ForcedFlatReason::StaleReference),
        "precondition: stale primary would force-flat before retry failover"
    );

    let mut retry_snapshot = active_snapshot_with_start("MKT-1", 1_000);
    retry_snapshot.published_at_ms = 1_220;
    strategy.apply_selection_snapshot(retry_snapshot);

    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(POLYRESEARCH_BACKUP_SOURCE_ID)
    );
    assert_eq!(strategy.active.reference_current_price, Some(101.0));
    assert_eq!(strategy.active.last_reference_ts_ms, Some(1_130));
    assert!(
        !strategy
            .active_forced_flat_reasons_at(1_220)
            .contains(&ForcedFlatReason::StaleReference),
        "retry failover should refresh reference state before stale-reference exit evaluation"
    );
}

#[test]
fn exit_submit_refreshes_reference_failover_before_forced_flat_check() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    reference_price.max_source_age_ms = 100;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.config.forced_flat_stale_reference_ms = 100;
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_120_u64 * NANOS_PER_MILLI_U64));
    let primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_100,
        1_105,
    );
    DataActor::on_data(&mut strategy, &primary).expect("primary quote should be handled");

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_140_u64 * NANOS_PER_MILLI_U64));
    let backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        101.0,
        1_130,
        1_135,
    );
    DataActor::on_data(&mut strategy, &backup)
        .expect("backup quote should be held while primary is still valid");
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(CHAINLINK_PRIMARY_SOURCE_ID)
    );

    strategy
        .try_submit_exit_order_for_trigger(
            1_220,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SelectionUpdate,
                1_220,
                None,
            ),
        )
        .expect("exit submit evaluation should not fail");

    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(POLYRESEARCH_BACKUP_SOURCE_ID)
    );
    assert_eq!(strategy.active.reference_current_price, Some(101.0));
    assert!(
        !strategy
            .active_forced_flat_reasons_at(1_220)
            .contains(&ForcedFlatReason::StaleReference),
        "exit submit path should refresh reference state before stale-reference evaluation"
    );
}

#[test]
fn cleared_reference_selection_preserves_last_reference_ts_for_forced_flat_grace() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price.max_source_age_ms = 100;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.config.forced_flat_stale_reference_ms = 300_000;
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_110_u64 * NANOS_PER_MILLI_U64));
    let update = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        TEST_REFERENCE_CURRENT_PRICE,
        1_100,
        1_110,
    );
    DataActor::on_data(&mut strategy, &update).expect("reference quote should be handled");
    assert_eq!(strategy.active.last_reference_ts_ms, Some(1_100));

    strategy.observe_current_reference_price_selection(1_000, 1_300, 1_250);

    assert_eq!(strategy.active.reference_current_price, None);
    assert_eq!(strategy.active.reference_current_price_source_id, None);
    assert_eq!(strategy.active.reference_current_price_ts_ms, None);
    assert_eq!(strategy.active.last_reference_ts_ms, Some(1_100));
    assert!(
        !strategy
            .active_forced_flat_reasons_at(1_250)
            .contains(&ForcedFlatReason::StaleReference),
        "clearing the selected quote must not erase the last observed reference timestamp"
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

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 1_200));
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
        "fresh primary quote must not be evicted by same-interval stale replay"
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
fn valid_out_of_order_reference_price_replay_does_not_replace_fresher_quote() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price.min_valid_sources = 2;
    reference_price.max_source_drift_bps = 50;
    reference_price.drift_policy = ReferencePriceDriftPolicy::Block;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let _cache = register_test_strategy(&mut strategy);

    let fresh_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_100,
        1_110,
    );
    DataActor::on_data(&mut strategy, &fresh_primary)
        .expect("fresh primary quote should be handled");

    let fresh_backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        100.1,
        1_105,
        1_115,
    );
    DataActor::on_data(&mut strategy, &fresh_backup).expect("fresh backup quote should be handled");

    let out_of_order_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        150.0,
        1_090,
        1_120,
    );
    DataActor::on_data(&mut strategy, &out_of_order_primary)
        .expect("valid out-of-order primary replay should be handled");

    let primary_quote = strategy
        .reference_price_quotes
        .get(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("fresh primary quote should survive valid out-of-order replay");
    assert_eq!(primary_quote.price(), 100.0);
    assert_eq!(primary_quote.observed_ts_ms(), 1_100);
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Available)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .and_then(|health| health.observed_ts_ms()),
        Some(1_100)
    );
    assert_eq!(strategy.active.reference_current_price, Some(100.0));
    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(CHAINLINK_PRIMARY_SOURCE_ID)
    );
    assert_eq!(strategy.active.reference_current_price_ts_ms, Some(1_100));
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
    assert_eq!(strategy.pricing.last_reference_current_price(), None);
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
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .and_then(|health| health.observed_ts_ms()),
        Some(1_100)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .and_then(|health| health.received_ts_ms()),
        Some(1_110)
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
fn stale_reference_price_status_survives_selection_block_refresh() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    reference_price
        .sources
        .get_mut(POLYRESEARCH_BACKUP_SOURCE_ID)
        .expect("polyresearch source should exist")
        .required = false;
    reference_price.min_valid_sources = 2;
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

    let fresh_backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        101.0,
        1_180,
        1_190,
    );
    DataActor::on_data(&mut strategy, &fresh_backup)
        .expect("backup quote should refresh block statuses");

    let primary_health = strategy
        .reference_price_source_health
        .get(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("primary health should exist");
    assert_eq!(primary_health.status(), ReferencePriceSourceStatus::Stale);
    assert_eq!(primary_health.observed_ts_ms(), Some(1_100));
    assert_eq!(primary_health.received_ts_ms(), Some(1_110));
}

#[test]
fn stale_source_status_refreshes_while_backup_remains_selectable() {
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
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);
    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_170_u64 * NANOS_PER_MILLI_U64));

    let fresh_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_140,
        1_145,
    );
    DataActor::on_data(&mut strategy, &fresh_primary)
        .expect("fresh primary quote should be handled");

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_200_u64 * NANOS_PER_MILLI_U64));
    let fresh_backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        101.0,
        1_180,
        1_185,
    );
    DataActor::on_data(&mut strategy, &fresh_backup)
        .expect("backup quote should be handled while primary is stale");

    assert_eq!(
        strategy.active.reference_current_price_source_id.as_deref(),
        Some(POLYRESEARCH_BACKUP_SOURCE_ID),
        "backup should remain selectable after the primary source ages out"
    );
    assert_eq!(strategy.active.reference_current_price, Some(101.0));
    let primary_health = strategy
        .reference_price_source_health
        .get(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("primary health should exist");
    assert_eq!(primary_health.status(), ReferencePriceSourceStatus::Stale);
    assert_eq!(primary_health.observed_ts_ms(), Some(1_140));
    assert_eq!(primary_health.received_ts_ms(), Some(1_145));
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(POLYRESEARCH_BACKUP_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::Available)
    );
}

#[test]
fn selector_block_clears_accepted_reference_price_state() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    reference_price.min_valid_sources = 1;
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
    assert_eq!(strategy.active.reference_current_price, Some(100.0));
    assert_eq!(strategy.pricing.last_reference_current_price(), Some(100.0));

    let drifting_backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        101.0,
        1_110,
        1_120,
    );
    DataActor::on_data(&mut strategy, &drifting_backup)
        .expect("drifting backup quote should be handled fail-closed");

    assert_eq!(strategy.active.reference_current_price, None);
    assert_eq!(strategy.active.reference_current_price_source_id, None);
    assert_eq!(strategy.active.reference_current_price_ts_ms, None);
    assert_eq!(strategy.active.last_reference_ts_ms, Some(1_100));
    assert_eq!(strategy.pricing.last_reference_current_price(), None);
    assert_eq!(strategy.pricing.last_reference_current_price_ts_ms(), None);
    assert_eq!(strategy.pricing.selected_pricing_spot().cloned(), None);
}

#[test]
fn stale_attempt_timestamp_survives_later_status_refresh() {
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
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);
    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_170_u64 * NANOS_PER_MILLI_U64));

    let fresh_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_140,
        1_145,
    );
    DataActor::on_data(&mut strategy, &fresh_primary)
        .expect("fresh primary quote should be handled");

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_250_u64 * NANOS_PER_MILLI_U64));
    let stale_primary_attempt = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.5,
        1_180,
        1_185,
    );
    DataActor::on_data(&mut strategy, &stale_primary_attempt)
        .expect("newer stale primary attempt should be handled");
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .and_then(|health| health.observed_ts_ms()),
        Some(1_180)
    );

    let fresh_backup = reference_price_update(
        POLYRESEARCH_BACKUP_SOURCE_ID,
        POLYRESEARCH_REFERENCE_PROVIDER,
        POLYRESEARCH_REFERENCE_SYMBOL,
        101.0,
        1_240,
        1_245,
    );
    DataActor::on_data(&mut strategy, &fresh_backup).expect("backup quote should refresh statuses");

    let primary_health = strategy
        .reference_price_source_health
        .get(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("primary health should exist");
    assert_eq!(primary_health.status(), ReferencePriceSourceStatus::Stale);
    assert_eq!(primary_health.observed_ts_ms(), Some(1_180));
    assert_eq!(primary_health.received_ts_ms(), Some(1_185));
}

#[test]
fn future_stale_attempt_status_survives_refresh_when_accepted_quote_is_still_fresh() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price
        .sources
        .get_mut(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("chainlink source should exist")
        .required = false;
    reference_price.max_source_age_ms = 5_000;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);
    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_170_u64 * NANOS_PER_MILLI_U64));

    let fresh_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_140,
        1_145,
    );
    DataActor::on_data(&mut strategy, &fresh_primary)
        .expect("fresh primary quote should be handled");

    let future_primary_attempt = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.5,
        1_180,
        1_185,
    );
    DataActor::on_data(&mut strategy, &future_primary_attempt)
        .expect("future primary attempt should be handled");

    let primary_health = strategy
        .reference_price_source_health
        .get(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("primary health should exist");
    assert_eq!(primary_health.status(), ReferencePriceSourceStatus::Stale);
    assert_eq!(primary_health.observed_ts_ms(), Some(1_180));
    assert_eq!(primary_health.received_ts_ms(), Some(1_185));
}

#[test]
fn out_of_order_stale_attempt_without_accepted_quote_preserves_newer_health_timestamp() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price.source_order = vec![CHAINLINK_PRIMARY_SOURCE_ID.to_string()];
    reference_price
        .sources
        .retain(|source_id, _source| source_id == CHAINLINK_PRIMARY_SOURCE_ID);
    reference_price.max_source_age_ms = 50;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);
    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_250_u64 * NANOS_PER_MILLI_U64));

    let newer_stale_attempt = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.5,
        1_180,
        1_185,
    );
    DataActor::on_data(&mut strategy, &newer_stale_attempt)
        .expect("newer stale primary attempt should be handled");

    let older_stale_attempt = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_170,
        1_175,
    );
    DataActor::on_data(&mut strategy, &older_stale_attempt)
        .expect("older stale primary attempt should not regress health");

    assert!(
        strategy
            .reference_price_quotes
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .is_none()
    );
    let primary_health = strategy
        .reference_price_source_health
        .get(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("primary health should exist");
    assert_eq!(primary_health.status(), ReferencePriceSourceStatus::Stale);
    assert_eq!(primary_health.observed_ts_ms(), Some(1_180));
    assert_eq!(primary_health.received_ts_ms(), Some(1_185));
}

#[test]
fn stale_selected_source_update_clears_accepted_reference_price_state() {
    let mut strategy = test_strategy();
    let mut reference_price = reference_price_config();
    reference_price.source_order = vec![CHAINLINK_PRIMARY_SOURCE_ID.to_string()];
    reference_price
        .sources
        .retain(|source_id, _source| source_id == CHAINLINK_PRIMARY_SOURCE_ID);
    reference_price.max_source_age_ms = 50;
    strategy.config.reference_current_price = Some(reference_price);
    strategy.active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);
    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_170_u64 * NANOS_PER_MILLI_U64));

    let fresh_primary = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.0,
        1_140,
        1_145,
    );
    DataActor::on_data(&mut strategy, &fresh_primary)
        .expect("fresh primary quote should be handled");
    assert_eq!(strategy.active.reference_current_price, Some(100.0));
    assert_eq!(strategy.pricing.last_reference_current_price(), Some(100.0));
    assert_eq!(
        strategy
            .pricing
            .selected_pricing_spot()
            .map(|spot| spot.price),
        Some(100.0)
    );

    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_250_u64 * NANOS_PER_MILLI_U64));
    let stale_primary_attempt = reference_price_update(
        CHAINLINK_PRIMARY_SOURCE_ID,
        CHAINLINK_REFERENCE_PROVIDER,
        CHAINLINK_REFERENCE_INSTRUMENT,
        100.5,
        1_180,
        1_185,
    );
    DataActor::on_data(&mut strategy, &stale_primary_attempt)
        .expect("newer stale primary attempt should be handled fail-closed");

    assert_eq!(strategy.active.reference_current_price, None);
    assert_eq!(strategy.active.reference_current_price_source_id, None);
    assert_eq!(strategy.active.reference_current_price_ts_ms, None);
    assert_eq!(strategy.active.last_reference_ts_ms, Some(1_140));
    assert_eq!(strategy.pricing.last_reference_current_price(), None);
    assert_eq!(strategy.pricing.last_reference_current_price_ts_ms(), None);
    assert_eq!(strategy.pricing.selected_pricing_spot().cloned(), None);
    let primary_health = strategy
        .reference_price_source_health
        .get(CHAINLINK_PRIMARY_SOURCE_ID)
        .expect("primary health should exist");
    assert_eq!(primary_health.status(), ReferencePriceSourceStatus::Stale);
    assert_eq!(primary_health.observed_ts_ms(), Some(1_180));
    assert_eq!(primary_health.received_ts_ms(), Some(1_185));
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
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .and_then(|health| health.observed_ts_ms()),
        Some(1_100)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(CHAINLINK_PRIMARY_SOURCE_ID)
            .and_then(|health| health.received_ts_ms()),
        Some(1_110)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(POLYRESEARCH_BACKUP_SOURCE_ID)
            .map(|health| health.status()),
        Some(ReferencePriceSourceStatus::DriftExceeded)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(POLYRESEARCH_BACKUP_SOURCE_ID)
            .and_then(|health| health.observed_ts_ms()),
        Some(1_100)
    );
    assert_eq!(
        strategy
            .reference_price_source_health
            .get(POLYRESEARCH_BACKUP_SOURCE_ID)
            .and_then(|health| health.received_ts_ms()),
        Some(1_110)
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
    assert_reference_price_subscription_for_asset(
        event,
        "BTC",
        action,
        source_id,
        provider,
        client_id,
        instrument_id,
        symbol,
    );
}

fn assert_reference_price_subscription_for_asset(
    event: &ReferencePriceSubscribeEvent,
    asset: &str,
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
    assert_eq!(event.data_type.identifier(), Some(asset));
    let metadata = event
        .data_type
        .metadata()
        .expect("reference price data type should carry metadata");
    assert_eq!(metadata.get_str(REFERENCE_PRICE_ASSET_PARAM), Some(asset));
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
        event.params.get_str(REFERENCE_PRICE_ASSET_PARAM),
        Some(asset)
    );
    assert_eq!(
        event.params.get_str(REFERENCE_PRICE_INSTRUMENT_ID_PARAM),
        instrument_id
    );
    assert_eq!(event.params.get_str(REFERENCE_PRICE_SYMBOL_PARAM), symbol);
}
