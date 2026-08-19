#![cfg(test)]

use super::*;

#[test]
fn chunk_universe_splits_into_consecutive_chunks_of_at_most_n() {
    let universe: Vec<u32> = (0..10).collect();
    assert_eq!(
        chunk_universe(&universe, 3),
        vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8], vec![9]],
        "chunks must be consecutive, in order, and at most chunk_size"
    );
}

#[test]
fn chunk_universe_returns_single_chunk_when_universe_fits() {
    assert_eq!(chunk_universe(&["a", "b"], 5), vec![vec!["a", "b"]]);
}

#[test]
fn chunk_universe_is_empty_for_empty_universe_or_zero_chunk_size() {
    assert!(chunk_universe::<u32>(&[], 4).is_empty());
    assert!(
        chunk_universe(&[1, 2, 3], 0).is_empty(),
        "chunk_size 0 must yield no chunks so the probe fails closed rather than panicking"
    );
}

#[test]
fn trade_chunk_count_probe_passes_only_at_or_above_m_with_positive_m() {
    assert!(
        !trade_chunk_count_probe_passed(0, 0),
        "m=0 must fail closed: requiring nothing proves nothing"
    );
    assert!(
        !trade_chunk_count_probe_passed(5, 0),
        "m=0 must fail closed even with fires"
    );
    assert!(!trade_chunk_count_probe_passed(9, 10), "below m must fail");
    assert!(
        trade_chunk_count_probe_passed(10, 10),
        "exactly m must pass"
    );
    assert!(trade_chunk_count_probe_passed(11, 10), "above m must pass");
}

fn readiness_trade_tick(instrument_id: InstrumentId, trade_id: &str) -> TradeTick {
    TradeTick::new(
        instrument_id,
        Price::from("1.00"),
        nautilus_model::types::Quantity::from("1.00"),
        AggressorSide::Buy,
        TradeId::from(trade_id),
        1.into(),
        1.into(),
    )
}

#[test]
fn chunk_count_handle_chunks_universe_and_walks_in_sorted_order() {
    let handle =
        BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_chunk_count_plan(
            ClientId::from("okx_data"),
            2,
            45,
            3,
            DataClientReadinessProbeMarketDataKind::Trade,
        );
    assert!(handle.is_chunk_count_mode());
    assert!(!handle.chunk_walk_started());

    handle.chunk_count_capture_universe(vec![
        InstrumentId::from("C-3.OKX"),
        InstrumentId::from("A-1.OKX"),
        InstrumentId::from("B-2.OKX"),
    ]);
    assert!(handle.chunk_walk_started());
    // 3 instruments at chunk_size 2 => 2 chunks; window threads through.
    assert_eq!(handle.chunk_walk_dims(), (2, 45));

    let first: Vec<String> = handle
        .chunk_count_next_chunk()
        .expect("first chunk")
        .iter()
        .map(|subscription| subscription.instrument_id.to_string())
        .collect();
    assert_eq!(
        first,
        vec!["A-1.OKX".to_string(), "B-2.OKX".to_string()],
        "the universe is walked in deterministic sorted order"
    );
    assert_eq!(
        handle.chunk_count_current_chunk().len(),
        2,
        "the current chunk tracks what is subscribed, for unsubscribe on advance"
    );

    assert_eq!(
        handle.chunk_count_next_chunk().expect("second chunk").len(),
        1,
        "the trailing chunk holds the remainder"
    );
    assert!(
        handle.chunk_count_next_chunk().is_none(),
        "the walk is exhausted after the last chunk"
    );
    assert!(
        !handle.chunk_count_passed(),
        "with no trades recorded the pass rule fails closed"
    );
}

#[test]
fn chunk_count_handle_passes_after_distinct_trade_markets_reach_m() {
    let handle =
        BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_chunk_count_plan(
            ClientId::from("okx_data"),
            3,
            45,
            2,
            DataClientReadinessProbeMarketDataKind::Trade,
        );
    handle.chunk_count_capture_universe(vec![
        InstrumentId::from("A-1.OKX"),
        InstrumentId::from("B-2.OKX"),
        InstrumentId::from("C-3.OKX"),
    ]);
    let chunk = handle.chunk_count_next_chunk().expect("first chunk");

    handle.record_trade(&readiness_trade_tick(chunk[0].instrument_id, "T-A1"));
    assert!(
        !handle.has_all_required_market_data(),
        "one distinct firing market is below m=2"
    );

    handle.record_trade(&readiness_trade_tick(chunk[0].instrument_id, "T-A2"));
    assert!(
        !handle.has_all_required_market_data(),
        "duplicate trades from the same market must not double-count"
    );

    handle.record_trade(&readiness_trade_tick(chunk[1].instrument_id, "T-B1"));
    assert!(
        handle.has_all_required_market_data(),
        "the trade chunk-count probe should pass once m distinct markets fire"
    );
}

#[test]
fn chunk_count_handle_fails_closed_when_universe_exhausts_below_m() {
    let handle =
        BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_chunk_count_plan(
            ClientId::from("okx_data"),
            1,
            45,
            2,
            DataClientReadinessProbeMarketDataKind::Trade,
        );
    handle.chunk_count_capture_universe(vec![InstrumentId::from("A-1.OKX")]);
    let chunk = handle.chunk_count_next_chunk().expect("first chunk");
    handle.record_trade(&readiness_trade_tick(chunk[0].instrument_id, "T-A1"));

    assert!(
        handle.chunk_count_next_chunk().is_none(),
        "the single-market universe is exhausted after one chunk"
    );
    let failure = handle
        .failure_error()
        .expect("exhausting below m must set a fail-closed reason");
    assert!(
        failure.contains("below required min_observed_targets=2"),
        "failure should explain the unmet m threshold: {failure}"
    );
    assert!(
        !handle.has_all_required_market_data(),
        "exhaustion below m must never satisfy readiness"
    );
}

#[test]
fn data_client_readiness_quote_plan_uses_client_owned_probe_targets() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include polymarket client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
        book_type: None,
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::Configured,
        max_metadata_quote_targets: None,
        allow_metadata_target_sampling: None,
        min_observed_targets: None,
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: Some(BTreeMap::from([(
            "configured_quote_probe".to_string(),
            DataClientReadinessProbeQuoteTargetBlock {
                instrument_id: InstrumentId::from("REFERENCE.POLYMARKET"),
            },
        )])),
    });

    let (required, ambiguous) =
        strategy_free_data_client_readiness_quote_subscription_plan(&loaded, "polymarket_main")
            .expect("client-owned readiness quote plan should build");

    assert!(ambiguous.is_empty());
    assert_eq!(required.len(), 1);
    assert_eq!(
        required[0].data_client_id,
        ClientId::from("polymarket_main")
    );
    assert_eq!(
        required[0].instrument_id,
        InstrumentId::from("REFERENCE.POLYMARKET")
    );
}

#[test]
fn data_client_readiness_metadata_response_probe_starts_pending_until_targets_arrive() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
        book_type: None,
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(2),
        allow_metadata_target_sampling: Some(false),
        min_observed_targets: None,
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("metadata-response readiness quote handle should build");

    assert!(
        !handle.has_all_required_quotes(),
        "metadata-response quote probes must not pass before same-run metadata installs targets"
    );
    let installed = handle.install_metadata_response_instrument_ids(vec![
        InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
        InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
    ]);

    assert_eq!(installed.len(), 2);
    assert!(
        !handle.has_all_required_quotes(),
        "installing targets should not pass the quote probe until quotes arrive"
    );
    for subscription in installed {
        handle
            .quotes
            .borrow_mut()
            .push(BoltV3StrategyFreeReferenceQuote {
                data_client_id: subscription.data_client_id.to_string(),
                instrument_id: subscription.instrument_id.to_string(),
                bid_price: 1.0,
                ask_price: 2.0,
                ts_event_unix_nanos: 1_000,
                ts_init_unix_nanos: 1_100,
                captured_at_unix_nanos: 1_200,
            });
    }

    assert!(
        handle.has_all_required_quotes(),
        "metadata-response quote probes should pass after every installed source-owned target has a quote"
    );
}

#[test]
fn data_client_readiness_metadata_response_probe_uses_actual_count_below_cap() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
        book_type: None,
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(20),
        allow_metadata_target_sampling: Some(false),
        min_observed_targets: None,
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("metadata-response readiness quote handle should build");
    let installed = handle.install_metadata_response_instrument_ids(vec![
        InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
        InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
        InstrumentId::from("CONFIGURED-THIRD.SOURCE"),
        InstrumentId::from("CONFIGURED-FOURTH.SOURCE"),
    ]);

    assert_eq!(
        installed.len(),
        4,
        "metadata_response probes must subscribe every source-owned target below the configured cap"
    );
    assert_eq!(
        handle.required_market_data_count(),
        installed.len(),
        "required observations must be based on the actual subscribed metadata targets"
    );

    for subscription in installed {
        handle
            .quotes
            .borrow_mut()
            .push(BoltV3StrategyFreeReferenceQuote {
                data_client_id: subscription.data_client_id.to_string(),
                instrument_id: subscription.instrument_id.to_string(),
                bid_price: 1.0,
                ask_price: 2.0,
                ts_event_unix_nanos: 1_000,
                ts_init_unix_nanos: 1_100,
                captured_at_unix_nanos: 1_200,
            });
    }

    assert!(
        handle.has_all_required_quotes(),
        "metadata-response quote probe should pass after the actual subscribed targets update"
    );
}

#[test]
fn data_client_readiness_metadata_response_probe_rejects_empty_metadata_universe() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
        book_type: None,
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(20),
        allow_metadata_target_sampling: Some(false),
        min_observed_targets: None,
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("metadata-response readiness quote handle should build");
    let installed = handle.install_metadata_response_instrument_ids(Vec::new());

    assert!(
        installed.is_empty(),
        "empty metadata_response universes must not install subscriptions"
    );
    let failure = handle
        .failure_error()
        .expect("empty metadata_response universe should fail closed");
    assert!(
        failure.contains("no source-owned instrument targets"),
        "failure should explain the empty metadata_response target set: {failure}"
    );
}

#[test]
fn data_client_readiness_metadata_response_probe_rejects_unbounded_metadata_universe() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
        book_type: None,
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(2),
        allow_metadata_target_sampling: Some(false),
        min_observed_targets: None,
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("metadata-response readiness quote handle should build");
    let installed = handle.install_metadata_response_instrument_ids(vec![
        InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
        InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
        InstrumentId::from("CONFIGURED-THIRD.SOURCE"),
    ]);

    assert!(
        installed.is_empty(),
        "metadata-response probes must not truncate a broad metadata universe into an arbitrary sample"
    );
    let failure = handle
        .failure_error()
        .expect("unbounded metadata universe should fail closed");
    assert!(
        failure.contains("max_metadata_quote_targets"),
        "failure should name the TOML-owned bound: {failure}"
    );
}

#[test]
fn data_client_readiness_metadata_response_probe_samples_when_explicitly_configured() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
        book_type: None,
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(3),
        allow_metadata_target_sampling: Some(true),
        min_observed_targets: None,
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("metadata-response readiness quote handle should build");
    let installed = handle.install_metadata_response_instrument_ids(vec![
        InstrumentId::from("CONFIGURED-C.SOURCE"),
        InstrumentId::from("CONFIGURED-A.SOURCE"),
        InstrumentId::from("CONFIGURED-E.SOURCE"),
        InstrumentId::from("CONFIGURED-B.SOURCE"),
        InstrumentId::from("CONFIGURED-D.SOURCE"),
    ]);

    assert_eq!(installed.len(), 3);
    assert_eq!(
        installed[0].instrument_id,
        InstrumentId::from("CONFIGURED-A.SOURCE")
    );
    assert_eq!(
        installed[1].instrument_id,
        InstrumentId::from("CONFIGURED-C.SOURCE")
    );
    assert_eq!(
        installed[2].instrument_id,
        InstrumentId::from("CONFIGURED-E.SOURCE")
    );
    assert!(handle.failure_error().is_none());
}

#[test]
fn data_client_readiness_metadata_response_probe_requires_all_metadata_quote_targets() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
        book_type: None,
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(3),
        allow_metadata_target_sampling: Some(false),
        min_observed_targets: None,
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("metadata-response readiness quote handle should build");
    let installed = handle.install_metadata_response_instrument_ids(vec![
        InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
        InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
        InstrumentId::from("CONFIGURED-THIRD.SOURCE"),
    ]);

    for subscription in installed.iter().take(1) {
        handle
            .quotes
            .borrow_mut()
            .push(BoltV3StrategyFreeReferenceQuote {
                data_client_id: subscription.data_client_id.to_string(),
                instrument_id: subscription.instrument_id.to_string(),
                bid_price: 1.0,
                ask_price: 2.0,
                ts_event_unix_nanos: 1_000,
                ts_init_unix_nanos: 1_100,
                captured_at_unix_nanos: 1_200,
            });
    }
    assert!(
        !handle.has_all_required_quotes(),
        "metadata-response quote probe must not pass before every same-run metadata target is observed"
    );

    let subscription = installed
        .get(1)
        .expect("second source-owned target should be installed");
    handle
        .quotes
        .borrow_mut()
        .push(BoltV3StrategyFreeReferenceQuote {
            data_client_id: subscription.data_client_id.to_string(),
            instrument_id: subscription.instrument_id.to_string(),
            bid_price: 1.0,
            ask_price: 2.0,
            ts_event_unix_nanos: 1_000,
            ts_init_unix_nanos: 1_100,
            captured_at_unix_nanos: 1_200,
        });

    assert!(
        !handle.has_all_required_quotes(),
        "metadata-response quote probe should still wait for the final same-run metadata target"
    );

    let subscription = installed
        .get(2)
        .expect("third source-owned target should be installed");
    handle
        .quotes
        .borrow_mut()
        .push(BoltV3StrategyFreeReferenceQuote {
            data_client_id: subscription.data_client_id.to_string(),
            instrument_id: subscription.instrument_id.to_string(),
            bid_price: 1.0,
            ask_price: 2.0,
            ts_event_unix_nanos: 1_000,
            ts_init_unix_nanos: 1_100,
            captured_at_unix_nanos: 1_200,
        });

    assert!(
        handle.has_all_required_quotes(),
        "metadata-response quote probe should pass after all same-run metadata targets are observed"
    );
}

#[test]
fn data_client_readiness_metadata_response_probe_accepts_book_deltas_when_configured() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
        book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(1),
        allow_metadata_target_sampling: Some(false),
        min_observed_targets: None,
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("metadata-response readiness book handle should build");
    let installed = handle.install_metadata_response_instrument_ids(vec![InstrumentId::from(
        "CONFIGURED-FIRST.SOURCE",
    )]);

    assert_eq!(installed.len(), 1);
    assert!(
        !handle.has_all_required_market_data(),
        "book probes must not pass before a source-owned book-delta event arrives"
    );
    let subscription = &installed[0];
    let delta = OrderBookDelta::new(
        subscription.instrument_id,
        BookAction::Add,
        BookOrder::new(
            OrderSide::Buy,
            Price::from("1.00"),
            Quantity::from("2.00"),
            1,
        ),
        0,
        0,
        1_000.into(),
        1_100.into(),
    );
    let deltas = OrderBookDeltas::new(subscription.instrument_id, vec![delta]);

    handle.record_book_deltas(&deltas, 1_200);

    assert!(
        handle.has_all_required_market_data(),
        "metadata-response book probes should pass after every installed source-owned target has book deltas"
    );
    assert_eq!(handle.book_evidence().deltas.len(), 1);
}

#[test]
fn data_client_readiness_metadata_response_book_probe_passes_at_min_observed_targets() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
        book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(5),
        allow_metadata_target_sampling: Some(true),
        min_observed_targets: Some(2),
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("metadata-response readiness book handle should build");
    let installed = handle.install_metadata_response_instrument_ids(vec![
        InstrumentId::from("CONFIGURED-A.SOURCE"),
        InstrumentId::from("CONFIGURED-B.SOURCE"),
        InstrumentId::from("CONFIGURED-C.SOURCE"),
        InstrumentId::from("CONFIGURED-D.SOURCE"),
        InstrumentId::from("CONFIGURED-E.SOURCE"),
    ]);
    assert_eq!(installed.len(), 5);

    let record_delta = |subscription: &StrategyFreeReferenceQuoteSubscription| {
        let delta = OrderBookDelta::new(
            subscription.instrument_id,
            BookAction::Add,
            BookOrder::new(
                OrderSide::Buy,
                Price::from("1.00"),
                Quantity::from("2.00"),
                1,
            ),
            0,
            0,
            1_000.into(),
            1_100.into(),
        );
        let deltas = OrderBookDeltas::new(subscription.instrument_id, vec![delta]);
        handle.record_book_deltas(&deltas, 1_200);
    };

    assert!(
        !handle.has_all_required_market_data(),
        "book probe must not pass before any sampled target streams a delta"
    );

    record_delta(&installed[0]);
    assert!(
        !handle.has_all_required_market_data(),
        "book probe must keep waiting below min_observed_targets (1 of required 2)"
    );

    record_delta(&installed[1]);
    assert!(
        handle.has_all_required_market_data(),
        "book probe should pass once min_observed_targets sampled targets stream fresh deltas, without requiring every illiquid sampled instrument to tick"
    );
}

#[test]
fn data_client_readiness_probe_rejects_zero_min_observed_targets() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
        book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(5),
        allow_metadata_target_sampling: Some(true),
        min_observed_targets: Some(0),
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    assert!(
        strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main").is_err(),
        "min_observed_targets=0 must fail closed: a probe that observes nothing proves nothing"
    );
}

#[test]
fn data_client_readiness_probe_fails_closed_when_min_observed_exceeds_sampled() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
        book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
        max_metadata_quote_targets: Some(5),
        allow_metadata_target_sampling: Some(true),
        min_observed_targets: Some(4),
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: None,
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("metadata-response readiness book handle should build");
    let installed = handle.install_metadata_response_instrument_ids(vec![
        InstrumentId::from("CONFIGURED-A.SOURCE"),
        InstrumentId::from("CONFIGURED-B.SOURCE"),
    ]);

    assert!(
        installed.is_empty(),
        "install must fail closed when min_observed_targets exceeds the sampled target count"
    );
    assert!(
        !handle.has_all_required_market_data(),
        "probe must not pass after min_observed_targets exceeds the sampled targets"
    );
}

#[test]
fn data_client_readiness_probe_times_out_without_market_data() {
    let mut loaded = fixture_loaded_config();
    let client = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include a data client");
    client.readiness_probe = Some(DataClientReadinessProbeBlock {
        market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
        book_type: None,
        quote_target_source: DataClientReadinessProbeQuoteTargetSource::Configured,
        max_metadata_quote_targets: None,
        allow_metadata_target_sampling: None,
        min_observed_targets: None,
        chunk_size: None,
        chunk_observation_window_seconds: None,
        quote_targets: Some(BTreeMap::from([(
            "configured_quote_probe".to_string(),
            DataClientReadinessProbeQuoteTargetBlock {
                instrument_id: InstrumentId::from("REFERENCE.POLYMARKET"),
            },
        )])),
    });

    let handle = strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
        .expect("configured readiness quote handle should build");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("test runtime should build");

    let timed_out = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_millis(1),
            handle.wait_for_all_required_quotes(),
        )
        .await
        .is_err()
    });

    assert!(timed_out, "no-data probe wait must time out");
    assert_eq!(handle.observed_market_data_count(), 0);
    assert_eq!(handle.required_market_data_count(), 1);
    assert!(
        !handle.has_all_required_market_data(),
        "zero observed updates must not satisfy the data-client probe"
    );
}
