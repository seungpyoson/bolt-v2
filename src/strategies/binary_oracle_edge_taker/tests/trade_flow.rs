#![cfg(test)]

use super::*;

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
    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        retained_instrument,
        Some(position_id),
        OrderSide::Buy,
    ));
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
