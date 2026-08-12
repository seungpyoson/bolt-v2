#![cfg(test)]

use super::*;

#[test]
fn outcome_book_state_applies_incremental_deltas_without_retaining_stale_levels() {
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    let mut state = OutcomeBookState::from_instrument_id(instrument_id);

    state.update_from_deltas(&book_deltas(
        instrument_id,
        &[
            (BookAction::Update, OrderSide::Buy, 0.43, 10.0),
            (BookAction::Update, OrderSide::Sell, 0.45, 12.0),
        ],
    ));
    assert_eq!(state.best_bid, Some(0.43));
    assert_eq!(state.best_ask, Some(0.45));

    state.update_from_deltas(&book_deltas(
        instrument_id,
        &[(BookAction::Delete, OrderSide::Buy, 0.43, 0.0)],
    ));

    assert_eq!(state.best_bid, None);
    assert_eq!(state.best_ask, Some(0.45));
    assert_eq!(state.liquidity_available, Some(12.0));
}

#[test]
fn entry_book_impact_cap_uses_configured_sell_side_book() {
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    let outcome_side = OutcomeSide::Up;
    strategy.config.entry_order.side = "sell".to_string();
    strategy.config.entry_order.position_side = "short".to_string();
    strategy.config.exit_order.side = "buy".to_string();
    strategy.config.exit_order.position_side = "short".to_string();
    strategy.config.book_impact_cap_bps = 0;
    set_configured_books_depth(
        &mut strategy,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.44, 7.0),
            (BookAction::Add, OrderSide::Buy, 0.44, 7.0),
            (BookAction::Add, OrderSide::Buy, 0.42, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.60, 100.0),
        ],
    );

    assert_eq!(strategy.visible_book_notional_cap(outcome_side), Some(3.08));
}

#[test]
fn post_only_entry_book_impact_cap_uses_passive_side_book() {
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    let outcome_side = OutcomeSide::Up;
    strategy.config.entry_order.is_post_only = true;
    strategy.config.book_impact_cap_bps = 0;
    set_configured_books_depth(
        &mut strategy,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.44, 7.0),
            (BookAction::Add, OrderSide::Buy, 0.44, 7.0),
            (BookAction::Add, OrderSide::Buy, 0.42, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.60, 100.0),
        ],
    );

    assert_eq!(strategy.visible_book_notional_cap(outcome_side), Some(3.08));
}

#[test]
fn book_impact_cap_is_derived_from_vwap_slippage_against_best_touch() {
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    let mut state = OutcomeBookState::from_instrument_id(instrument_id);
    state.update_from_deltas(&book_deltas(
        instrument_id,
        &[
            (BookAction::Add, OrderSide::Buy, 0.49, 10.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 10.0),
            (BookAction::Add, OrderSide::Sell, 0.60, 10.0),
        ],
    ));

    let zero_bps = state
        .max_execution_within_vwap_slippage_bps(OrderSide::Buy, 0)
        .expect("best-touch-only size should exist");
    let one_hundred_bps = state
        .max_execution_within_vwap_slippage_bps(OrderSide::Buy, 100)
        .expect("partial next-level size should exist");
    let loose = state
        .max_execution_within_vwap_slippage_bps(OrderSide::Buy, 5_000)
        .expect("full displayed size should exist");

    assert_eq!(zero_bps.quantity, 10.0);
    assert!(one_hundred_bps.quantity > zero_bps.quantity);
    assert!(one_hundred_bps.quantity < loose.quantity);
    assert_eq!(loose.quantity, 20.0);
    assert!(one_hundred_bps.vwap_price > zero_bps.vwap_price);
}

#[test]
fn book_impact_cap_config_changes_sizing_decision() {
    let outcome_side = OutcomeSide::Up;
    let mut loose = ready_to_trade_strategy_with_bound_economics();
    loose.config.book_impact_cap_bps = 5_000;
    let loose_instrument_id = loose
        .instrument_id_for_side(outcome_side)
        .expect("fixture should configure the UP outcome instrument");
    loose.active.books.update_from_deltas(&book_deltas(
        loose_instrument_id,
        &[
            (BookAction::Add, OrderSide::Buy, 0.49, 10.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 10.0),
            (BookAction::Add, OrderSide::Sell, 0.60, 10.0),
        ],
    ));

    let mut tight = ready_to_trade_strategy_with_bound_economics();
    tight.config.book_impact_cap_bps = 0;
    let tight_instrument_id = tight
        .instrument_id_for_side(outcome_side)
        .expect("fixture should configure the UP outcome instrument");
    tight.active.books.update_from_deltas(&book_deltas(
        tight_instrument_id,
        &[
            (BookAction::Add, OrderSide::Buy, 0.49, 10.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 10.0),
            (BookAction::Add, OrderSide::Sell, 0.60, 10.0),
        ],
    ));

    let loose_cap = loose.visible_book_notional_cap(outcome_side);
    let tight_cap = tight.visible_book_notional_cap(outcome_side);

    assert!(
        loose_cap
            .zip(tight_cap)
            .is_some_and(|(loose_cap, tight_cap)| tight_cap < loose_cap),
        "tighter impact cap should reduce the derived notional cap"
    );
}

#[test]
fn rotated_position_uses_position_book_for_thin_book_forced_flat() {
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    let position_outcome_side = OutcomeSide::Up;
    let position_instrument = InstrumentId::from("condition-MKT-A-UP.POLYMARKET");
    let mut tracked_book = OutcomeBookState::from_instrument_id(position_instrument);
    tracked_book.last_observed_instrument_id = Some(position_instrument);
    tracked_book.best_bid = Some(0.430);
    tracked_book.best_ask = Some(0.450);
    tracked_book.liquidity_available = Some(5.0);
    let open_position = OpenPositionState {
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-A".to_string()),
            Some(position_outcome_side),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id: position_instrument,
        position_id: PositionId::from("P-THIN-001"),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(5.0, 2),
        avg_px_open: 0.450,
        book: tracked_book,
    };
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
    strategy.active.books.up.liquidity_available = Some(5_000.0);
    strategy.active.books.down.liquidity_available = Some(5_000.0);

    let decision = strategy.exit_intent_decision_at(2_000);

    assert!(
        decision
            .forced_flat_reasons
            .contains(&ForcedFlatReason::ThinBook)
    );
    assert_eq!(decision.order_side, Some(OrderSide::Sell));
    assert_eq!(decision.instrument_id, Some(position_instrument));
}

#[test]
fn market_switch_replaces_both_outcome_book_subscriptions() {
    let mut strategy = test_strategy();

    strategy.apply_selection_snapshot(active_snapshot("A"));
    strategy.book_subscription_events.clear();

    strategy.apply_selection_snapshot(active_snapshot("B"));

    assert_eq!(
        strategy.book_subscription_events,
        vec![
            BookSubscriptionEvent::unsubscribe(InstrumentId::from("condition-A-A-UP.POLYMARKET")),
            BookSubscriptionEvent::unsubscribe(InstrumentId::from("condition-A-A-DOWN.POLYMARKET")),
            BookSubscriptionEvent::subscribe(InstrumentId::from("condition-B-B-UP.POLYMARKET")),
            BookSubscriptionEvent::subscribe(InstrumentId::from("condition-B-B-DOWN.POLYMARKET")),
        ]
    );
}

#[test]
fn entry_gate_blocks_when_active_outcome_book_is_strictly_crossed() {
    // Normal book: bid < ask is not crossed.
    let normal = ready_to_trade_strategy();
    assert!(normal.active.books.up.best_bid.unwrap() < normal.active.books.up.best_ask.unwrap());
    assert!(
        !normal
            .entry_gate_decision_at(2_000)
            .blocked_by
            .contains(&EntryBlockReason::BookCrossed),
        "normal bid<ask book must not trip the crossed-book guard"
    );

    // Up book strictly crossed: best_bid > best_ask blocks entry.
    let mut up_crossed = ready_to_trade_strategy();
    up_crossed.active.books.up.best_bid = Some(0.46);
    up_crossed.active.books.up.best_ask = Some(0.45);
    assert!(
        up_crossed
            .entry_gate_decision_at(2_000)
            .blocked_by
            .contains(&EntryBlockReason::BookCrossed),
        "strictly crossed up book must block entry"
    );

    // Down book strictly crossed is detected too (gate treats both books as active).
    let mut down_crossed = ready_to_trade_strategy();
    down_crossed.active.books.down.best_bid = Some(0.46);
    down_crossed.active.books.down.best_ask = Some(0.45);
    assert!(
        down_crossed
            .entry_gate_decision_at(2_000)
            .blocked_by
            .contains(&EntryBlockReason::BookCrossed),
        "strictly crossed down book must block entry"
    );

    // Locked book (bid == ask) is intentionally not crossed.
    let mut locked = ready_to_trade_strategy();
    locked.active.books.up.best_bid = Some(0.45);
    locked.active.books.up.best_ask = Some(0.45);
    assert!(
        !locked
            .entry_gate_decision_at(2_000)
            .blocked_by
            .contains(&EntryBlockReason::BookCrossed),
        "locked bid==ask book must not trip the crossed-book guard"
    );
}

#[test]
fn task5_missing_liquidity_is_thin_book() {
    let reasons = evaluate_forced_flat_predicates(&ForcedFlatInputs {
        frozen: false,
        metadata_matches_selection: true,
        last_reference_ts_ms: Some(1_000),
        now_ms: 1_250,
        stale_reference_after_ms: 1_500,
        liquidity_available: None,
        min_liquidity_required: 100.0,
        fast_venue_incoherent: false,
    });

    assert_eq!(reasons, vec![ForcedFlatReason::ThinBook]);
}
