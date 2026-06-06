#![cfg(test)]

use super::*;

#[test]
fn signed_trade_flow_observe_appends_signed_price_and_size() {
    let mut config = test_strategy().config.clone();
    config.trade_flow_window_secs = 30;
    config.trade_flow_max_samples = 100;
    let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&config));

    flow.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.42,
        3.0,
        AggressorSide::Buyer,
        1_000,
    ));
    flow.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.41,
        2.0,
        AggressorSide::Seller,
        1_500,
    ));
    flow.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.40,
        1.0,
        AggressorSide::NoAggressor,
        2_000,
    ));

    assert_eq!(flow.len(), 3);
    assert!(!flow.is_empty());
    let samples: Vec<SignedTrade> = flow.samples().iter().copied().collect();
    // Prices/sizes are compared through the same fixed-point round-trip the
    // production path uses, so the test does not depend on literal f64 bits.
    assert_eq!(
        samples,
        vec![
            SignedTrade {
                ts_ms: 1_000,
                aggressor: AggressorSide::Buyer,
                price: Price::new(0.42, 2).as_f64(),
                size: Quantity::new(3.0, 0).as_f64(),
            },
            SignedTrade {
                ts_ms: 1_500,
                aggressor: AggressorSide::Seller,
                price: Price::new(0.41, 2).as_f64(),
                size: Quantity::new(2.0, 0).as_f64(),
            },
            SignedTrade {
                ts_ms: 2_000,
                aggressor: AggressorSide::NoAggressor,
                price: Price::new(0.40, 2).as_f64(),
                size: Quantity::new(1.0, 0).as_f64(),
            },
        ]
    );
}

#[test]
fn signed_trade_flow_drops_out_of_order_and_duplicate_timestamps() {
    // The buffer doc promises samples "oldest first" and that it mirrors
    // RealizedVolEstimator, which rejects non-monotonic observations. A trade
    // whose timestamp is not strictly greater than the latest retained sample
    // would otherwise corrupt ordering and the time-window eviction cutoff, so
    // it must be dropped.
    let mut config = test_strategy().config.clone();
    config.trade_flow_window_secs = 600;
    config.trade_flow_max_samples = 100;
    let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&config));

    flow.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.50,
        1.0,
        AggressorSide::Buyer,
        1_000,
    ));
    flow.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.51,
        1.0,
        AggressorSide::Buyer,
        2_000,
    ));
    // Out-of-order: an earlier timestamp than the latest retained sample.
    flow.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.52,
        1.0,
        AggressorSide::Seller,
        1_500,
    ));
    // Duplicate: equal to the latest retained timestamp.
    flow.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.53,
        1.0,
        AggressorSide::Seller,
        2_000,
    ));

    assert_eq!(flow.len(), 2);
    assert_eq!(
        flow.samples()
            .iter()
            .map(|trade| trade.ts_ms)
            .collect::<Vec<_>>(),
        vec![1_000, 2_000],
        "out-of-order and duplicate-timestamp trades must be dropped to keep \
         the buffer monotonic"
    );
}

#[test]
fn signed_trade_flow_samples_within_excludes_trades_aged_out_by_caller_clock() {
    // Eviction only runs inside `observe`, so in a quiet market `samples()`
    // can still hold trades that have aged out of the window. A point-in-time
    // consumer reads through `samples_within(now)`, which filters against the
    // caller's clock.
    let mut config = test_strategy().config.clone();
    config.trade_flow_window_secs = 10; // 10_000ms window
    config.trade_flow_max_samples = 100;
    let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&config));

    flow.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.50,
        1.0,
        AggressorSide::Buyer,
        1_000,
    ));
    flow.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.51,
        1.0,
        AggressorSide::Buyer,
        5_000,
    ));

    // observe-time eviction at 5_000ms (cutoff 5_000 - 10_000 saturates to 0)
    // retains both; the raw buffer is not filtered by the caller's clock.
    assert_eq!(flow.len(), 2);

    // As of now = 20_000ms the window is [10_000, 20_000]; both trades have
    // aged out, so a point-in-time read reports none.
    assert!(flow.samples_within(20_000).next().is_none());

    // As of now = 12_000ms the window is [2_000, 12_000]: the 5_000ms trade is
    // in-window, the 1_000ms trade has aged out.
    assert_eq!(
        flow.samples_within(12_000)
            .map(|trade| trade.ts_ms)
            .collect::<Vec<_>>(),
        vec![5_000]
    );
}

#[test]
fn signed_trade_flow_evicts_by_window_then_caps_by_max_samples() {
    // Window eviction: a 10-second window drops trades older than now - window.
    let mut window_config = test_strategy().config.clone();
    window_config.trade_flow_window_secs = 10;
    window_config.trade_flow_max_samples = 100;
    let mut windowed = SignedTradeFlow::from_config(&signed_trade_flow_config(&window_config));

    windowed.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.50,
        1.0,
        AggressorSide::Buyer,
        1_000,
    ));
    windowed.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.51,
        1.0,
        AggressorSide::Buyer,
        5_000,
    ));
    // Latest trade at 15_000ms makes the window cutoff 5_000ms; the 1_000ms
    // sample is strictly older than the cutoff and is evicted, the 5_000ms
    // sample sits exactly on the cutoff and is retained.
    windowed.observe(&trade_tick_with_aggressor(
        "condition-A-A-UP.POLYMARKET",
        0.52,
        1.0,
        AggressorSide::Seller,
        15_000,
    ));

    assert_eq!(windowed.len(), 2);
    assert_eq!(
        windowed
            .samples()
            .iter()
            .map(|trade| trade.ts_ms)
            .collect::<Vec<_>>(),
        vec![5_000, 15_000]
    );

    // Count cap: with a wide window, max_samples bounds retained trades and
    // drops the oldest first.
    let mut cap_config = test_strategy().config.clone();
    cap_config.trade_flow_window_secs = 600;
    cap_config.trade_flow_max_samples = 2;
    let mut capped = SignedTradeFlow::from_config(&signed_trade_flow_config(&cap_config));

    for index in 0..4_u64 {
        capped.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.50,
            1.0,
            AggressorSide::Buyer,
            1_000 + index,
        ));
    }

    assert_eq!(capped.len(), 2);
    assert_eq!(
        capped
            .samples()
            .iter()
            .map(|trade| trade.ts_ms)
            .collect::<Vec<_>>(),
        vec![1_002, 1_003]
    );
}

#[test]
fn on_trade_routes_to_subscribed_instrument_and_ignores_untracked() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot("A"));

    let up_instrument = "condition-A-A-UP.POLYMARKET";
    let down_instrument = "condition-A-A-DOWN.POLYMARKET";
    let untracked_instrument = "condition-Z-Z-UP.POLYMARKET";

    strategy
        .on_trade(&trade_tick_with_aggressor(
            up_instrument,
            0.42,
            2.0,
            AggressorSide::Buyer,
            1_000,
        ))
        .expect("trade on subscribed instrument should process");
    strategy
        .on_trade(&trade_tick_with_aggressor(
            untracked_instrument,
            0.99,
            1.0,
            AggressorSide::Seller,
            1_100,
        ))
        .expect("trade on untracked instrument should be ignored without error");

    let up_flow = strategy
        .active
        .trade_flow
        .get(&InstrumentId::from(up_instrument))
        .expect("subscribed up instrument should have a trade-flow buffer");
    assert_eq!(up_flow.len(), 1);
    assert_eq!(up_flow.samples()[0].aggressor, AggressorSide::Buyer);

    let down_flow = strategy
        .active
        .trade_flow
        .get(&InstrumentId::from(down_instrument))
        .expect("subscribed down instrument should have a trade-flow buffer");
    assert!(down_flow.is_empty());

    assert!(
        !strategy
            .active
            .trade_flow
            .contains_key(&InstrumentId::from(untracked_instrument)),
        "untracked instrument must not create a trade-flow buffer"
    );
}

#[test]
fn market_switch_creates_and_removes_trade_flow_buffers_in_lockstep_with_books() {
    let mut strategy = test_strategy();

    strategy.apply_selection_snapshot(active_snapshot("A"));
    assert_eq!(
        strategy.active.trade_flow.keys().collect::<Vec<_>>(),
        vec![
            &InstrumentId::from("condition-A-A-DOWN.POLYMARKET"),
            &InstrumentId::from("condition-A-A-UP.POLYMARKET"),
        ]
    );

    strategy.apply_selection_snapshot(active_snapshot("B"));
    assert_eq!(
        strategy.active.trade_flow.keys().collect::<Vec<_>>(),
        vec![
            &InstrumentId::from("condition-B-B-DOWN.POLYMARKET"),
            &InstrumentId::from("condition-B-B-UP.POLYMARKET"),
        ]
    );
}

#[test]
fn same_market_refresh_preserves_accumulated_trade_flow() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("A", 1_000));

    strategy
        .on_trade(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.42,
            2.0,
            AggressorSide::Buyer,
            1_200,
        ))
        .expect("trade on subscribed instrument should process");

    // A new interval for the same market must not discard accumulated flow.
    strategy.apply_selection_snapshot(active_snapshot_with_start("A", 2_000));

    let up_flow = strategy
        .active
        .trade_flow
        .get(&InstrumentId::from("condition-A-A-UP.POLYMARKET"))
        .expect("same-market refresh should retain the trade-flow buffer");
    assert_eq!(up_flow.len(), 1);
}

#[test]
fn real_market_change_preserves_retained_instrument_trade_flow() {
    // Regression: on a real market change (preserve_books == false), the
    // trade_flow restore must be unconditional. An instrument that is
    // RETAINED across the rotation (here, the open position's instrument,
    // tracked via `tracked_position_instrument_id`) is touched by neither
    // `unsubscribe_missing_books` (it did not change away) nor
    // `subscribe_new_books` (it is not new), so dropping its buffer in
    // `apply_selection_snapshot_to_active` would silently lose all
    // accumulated SignedTradeFlow for the live position.
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-A");
    let position_id = PositionId::from("P-A");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let retained_instrument = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    // Rotate to a different market, then fill: the position materializes on
    // the original (now non-active) instrument, which becomes the tracked
    // position instrument with its own freshly subscribed trade-flow buffer.
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
    strategy
        .on_order_filled(&order_filled_event(
            entry_client_order_id,
            retained_instrument,
            position_id,
        ))
        .expect("fill bookkeeping should succeed");
    assert_eq!(
        strategy.book_subscriptions.tracked_position_instrument_id,
        Some(retained_instrument),
        "open position instrument must be the retained tracked instrument",
    );
    assert!(
        strategy
            .active
            .trade_flow
            .contains_key(&retained_instrument),
        "fill must subscribe a trade-flow buffer for the tracked instrument",
    );

    // Accumulate signed trade flow on the retained instrument.
    strategy
        .on_trade(&trade_tick_with_aggressor(
            retained_instrument.to_string().as_str(),
            0.42,
            2.0,
            AggressorSide::Buyer,
            2_200,
        ))
        .expect("trade on the tracked instrument should process");
    assert!(
        !strategy
            .active
            .trade_flow
            .get(&retained_instrument)
            .expect("tracked instrument buffer must exist before rotation")
            .is_empty(),
        "seeded trade should be retained before the rotation",
    );

    // Real market change (preserve_books == false). The retained instrument
    // is unchanged across the rotation, so its buffer must survive.
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-3", 3_000));
    assert_eq!(
        strategy.book_subscriptions.tracked_position_instrument_id,
        Some(retained_instrument),
        "instrument should remain the tracked instrument across the rotation",
    );
    let retained_flow = strategy
        .active
        .trade_flow
        .get(&retained_instrument)
        .expect("retained instrument trade-flow buffer must survive a real market change");
    assert!(
        !retained_flow.is_empty(),
        "retained instrument must keep its accumulated SignedTradeFlow across a real market change",
    );
    assert_eq!(retained_flow.len(), 1);
}
